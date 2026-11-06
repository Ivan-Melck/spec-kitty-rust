//! ONE canonical ambient write-observation context.
//!
//! The pinned reference keeps its write observer in a `contextvars.ContextVar`
//! (`kernel/atomic.py::_WRITE_OBSERVER`): `observe_writes` captures the observer
//! the current context already had, installs a chain that calls the outer
//! observer *first* and the new one second, and returns a reset token;
//! `stop_observing_writes` resets that token; `notify_written` snapshots the
//! current observer and calls it with the exact `(path, bytes)` it was handed —
//! no normalisation, `stat`, read or existence check. A context that installed
//! no observer is a no-op, and installing or stopping an observer writes nothing
//! to the filesystem.
//!
//! This module is the single Native authority for that behaviour. It is
//! deliberately *not* a second registry, a per-writer ledger, a global callback
//! `Mutex`, or a bare thread-local that pretends to be a `ContextVar`:
//!
//! * the ambient binding is an execution-context stack (a `thread_local!`, the
//!   only std-safe binding), but the observable *value* and the reset *identity*
//!   live in an explicit [`WriteContext`] handle;
//! * a new thread starts with an empty context (Source's per-thread default),
//!   while [`WriteContext::capture`] produces an explicit copy that can be
//!   entered anywhere ([`WriteContext::enter`]) and whose observer value is the
//!   snapshot taken at capture time;
//! * [`WriteContext::clone`] aliases the same context (same token identity), it
//!   is not a deep copy;
//! * [`bind_write_context`] snapshots the context for a future at *task
//!   creation* and re-binds it around every `poll`, so interleaved captured
//!   futures cannot leak an observer into one another and no ambient state
//!   survives a poll;
//! * callbacks are held in immutable `Arc` chains, so no registry, context or
//!   mutex borrow is held while user code runs. An observer may install, stop or
//!   recursively notify.
//!
//! ## Tokens (Source `ContextVar` token semantics)
//!
//! A [`WriteObserverToken`] carries the *real* context identity (an `Arc`
//! compared with [`Arc::ptr_eq`]) and the value the variable held before the
//! install. It is opaque by construction — there is no synthetic UUID, ordinal
//! or clock pretending to be Source identity — and it resets exactly once:
//!
//! * a second reset is a `RuntimeError`-class [`WriteContextError::TokenAlreadyUsed`];
//! * resetting from a different context is a `ValueError`-class
//!   [`WriteContextError::ForeignContext`];
//! * **out-of-order resets are real**, never a fake LIFO guard: resetting an
//!   outer token while an inner observer is installed restores the outer
//!   token's previous value (dropping the inner observer), and resetting the
//!   inner token afterwards re-installs the outer chain. That resurrection is
//!   the reference behaviour, not a bug to paper over.
//!
//! The same explicit context cannot be entered recursively or concurrently:
//! [`WriteContextError::ContextBusy`] (`RuntimeError`-class) is refused instead
//! of silently sharing one context between two live scopes.
//!
//! ## Errors (Source exception classes)
//!
//! A Source observer may raise `OSError`, `ValueError` or any custom exception,
//! and callers catch those classes differently. [`WriteObserverError`] therefore
//! keeps a typed cause (never a scalar-only message): [`WriteObserverError::os`]
//! stays an `io::Error` and is `OSError`-class, [`WriteObserverError::value`] is
//! `ValueError`-class (an `except OSError` caller does **not** catch it), and
//! [`WriteObserverError::custom`] keeps the arbitrary error object. A
//! message-only failure stays unclassified ([`WriteObserverError::message`]),
//! which is also how the legacy, message-only
//! [`super::WriteListenerFailure`] bridges into this type without inventing a
//! class from a string. The exception *wire format* for the Native boundary is
//! OPEN; only the class and phase are declared.
//!
//! ## Deliberate limits (not waivers)
//!
//! This is a propagation primitive, not an async runtime: there is no `Tokio`
//! dependency and no task registry. Futures are bound with the std
//! [`std::future::Future`] trait and a safe `Pin<Box<_>>` wrapper. The explicit
//! entry flag is process-local; cross-process observers and the durable
//! finalize/bake/status write-ledger adapters are consumer integrations that
//! remain OPEN and are not implemented here.

use std::cell::RefCell;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::io;
use std::marker::PhantomData;
use std::path::Path;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};

/// The Source observer signature: `(path, bytes) -> None`.
///
/// `Arc` (not `&mut`) so the caller never holds a context or registry borrow
/// while the observer runs; observers may re-enter this module.
pub type ObserverCallback =
    Arc<dyn Fn(&Path, &[u8]) -> Result<(), WriteObserverError> + Send + Sync>;

/// An immutable outer-first observer chain (Source's nested `both` closures).
type Chain = Arc<Vec<ObserverCallback>>;

// ---------------------------------------------------------------------------
// Typed observer failures
// ---------------------------------------------------------------------------

/// A typed failure raised by an ambient write observer.
///
/// The variants keep the reference's exception *classes* apart instead of
/// collapsing every observer into an `OSError` or deleting a real `io::Error`
/// behind a string.
#[derive(Debug)]
pub enum WriteObserverError {
    /// Source `OSError` (an `except OSError` caller catches it).
    Os(io::Error),
    /// Source `ValueError` (an `except OSError` caller does **not** catch it).
    ValueError(String),
    /// Any other Source exception object, retained with its own source chain.
    Custom(Box<dyn Error + Send + Sync>),
    /// A message-only failure whose class is unknown (legacy listener bridge).
    Message(String),
}

impl WriteObserverError {
    /// An `OSError`-class failure.
    pub fn os(error: io::Error) -> Self {
        Self::Os(error)
    }

    /// A `ValueError`-class failure.
    pub fn value(message: impl Into<String>) -> Self {
        Self::ValueError(message.into())
    }

    /// A custom Source exception object.
    pub fn custom(error: impl Error + Send + Sync + 'static) -> Self {
        Self::Custom(Box::new(error))
    }

    /// A message-only failure with no declared class.
    pub fn from_message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// The observer's message (the `Display` form of the cause).
    pub fn message(&self) -> String {
        match self {
            Self::Os(error) => error.to_string(),
            Self::ValueError(message) | Self::Message(message) => message.clone(),
            Self::Custom(error) => error.to_string(),
        }
    }

    /// Whether Source would raise this as an `OSError` subclass.
    pub fn is_os_error(&self) -> bool {
        matches!(self, Self::Os(_))
    }

    /// Whether Source would raise this as a `ValueError`.
    pub fn is_value_error(&self) -> bool {
        matches!(self, Self::ValueError(_))
    }

    /// The retained `io::Error`, when this is an `OSError`-class failure.
    pub fn io_error(&self) -> Option<&io::Error> {
        match self {
            Self::Os(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for WriteObserverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl Error for WriteObserverError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Os(error) => Some(error),
            Self::Custom(error) => Some(error.as_ref()),
            Self::ValueError(_) | Self::Message(_) => None,
        }
    }
}

impl From<io::Error> for WriteObserverError {
    fn from(error: io::Error) -> Self {
        Self::Os(error)
    }
}

// ---------------------------------------------------------------------------
// Context/token failures
// ---------------------------------------------------------------------------

/// A refusal from the ambient context itself (Source `ContextVar`/`Context.run`
/// exception classes).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteContextError {
    /// The token was already reset (`RuntimeError`: "Token has already been used once").
    TokenAlreadyUsed,
    /// The token belongs to a different context (`ValueError`: "Token was created
    /// in a different Context").
    ForeignContext,
    /// The same explicit context is already entered, recursively or concurrently
    /// (`RuntimeError`: "cannot enter context: it is already entered").
    ContextBusy,
}

impl WriteContextError {
    /// Whether Source would raise this as a `RuntimeError`.
    pub const fn is_runtime_error(self) -> bool {
        matches!(self, Self::TokenAlreadyUsed | Self::ContextBusy)
    }

    /// Whether Source would raise this as a `ValueError`.
    pub const fn is_value_error(self) -> bool {
        matches!(self, Self::ForeignContext)
    }
}

impl fmt::Display for WriteContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TokenAlreadyUsed => "Token has already been used once",
            Self::ForeignContext => "Token was created in a different Context",
            Self::ContextBusy => "cannot enter context: it is already entered",
        })
    }
}

impl Error for WriteContextError {}

// ---------------------------------------------------------------------------
// The context value
// ---------------------------------------------------------------------------

struct ContextValue {
    /// Source `_WRITE_OBSERVER.get()`.
    chain: Mutex<Option<Chain>>,
    /// Set only by [`WriteContext::enter`]; guards refuse a second entry.
    entered: AtomicBool,
}

impl ContextValue {
    fn empty() -> Self {
        Self {
            chain: Mutex::new(None),
            entered: AtomicBool::new(false),
        }
    }

    fn snapshot(&self) -> Option<Chain> {
        lock(&self.chain).clone()
    }

    fn replace(&self, value: Option<Chain>) -> Option<Chain> {
        std::mem::replace(&mut *lock(&self.chain), value)
    }
}

/// An explicit ambient write-observation context.
///
/// `Clone` aliases the same context (same reset identity); [`Self::capture`]
/// makes a distinct context whose value is a snapshot of this one.
#[derive(Clone)]
pub struct WriteContext {
    value: Arc<ContextValue>,
}

impl Default for WriteContext {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteContext {
    /// A new, empty context (Source's default `None` observer).
    pub fn new() -> Self {
        Self {
            value: Arc::new(ContextValue::empty()),
        }
    }

    /// Snapshot this context's observer value into a **distinct** context.
    ///
    /// The callbacks themselves are shared (their captured state is the same
    /// objects), but the copy has its own reset identity and its own mutable
    /// value: installing or stopping an observer on the copy never changes this
    /// context, and a token created here is foreign to it.
    pub fn capture(&self) -> Self {
        Self {
            value: Arc::new(ContextValue {
                chain: Mutex::new(self.value.snapshot()),
                entered: AtomicBool::new(false),
            }),
        }
    }

    /// Bind this context as the ambient context for the returned scope.
    ///
    /// The guard restores the previous ambient binding on return, `?`-error and
    /// unwind. The same context cannot be entered twice in a live scope
    /// (recursively or from another thread): that is [`WriteContextError::ContextBusy`].
    pub fn enter(&self) -> Result<WriteContextGuard, WriteContextError> {
        if self.value.entered.swap(true, Ordering::AcqRel) {
            return Err(WriteContextError::ContextBusy);
        }
        Ok(self.push())
    }

    /// Whether this context currently has an observer chain installed.
    pub fn is_observing(&self) -> bool {
        self.value.snapshot().is_some()
    }

    fn push(&self) -> WriteContextGuard {
        STACK.with(|stack| stack.borrow_mut().push(self.clone()));
        WriteContextGuard {
            context: self.clone(),
            _thread_bound: PhantomData,
        }
    }
}

fn same_context(left: &WriteContext, right: &WriteContext) -> bool {
    Arc::ptr_eq(&left.value, &right.value)
}

/// Scope guard for [`WriteContext::enter`] / [`bind_write_context`].
///
/// Dropping restores the previously ambient context, including on unwind.
/// A guard owns a binding on THIS thread, so it must not move to another:
///
/// ```compile_fail
/// use sk_kernel::source_writer::write_observer::WriteContext;
/// let guard = WriteContext::new().enter().unwrap();
/// std::thread::spawn(move || drop(guard));
/// ```
#[must_use = "the context is only bound while the guard is alive"]
pub struct WriteContextGuard {
    context: WriteContext,
    _thread_bound: PhantomData<Rc<()>>,
}

impl Drop for WriteContextGuard {
    fn drop(&mut self) {
        // Rust callers can drop outer guards before inner guards. Remove only
        // this binding; never pop somebody else's still-live context.
        STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(position) = stack
                .iter()
                .rposition(|item| same_context(item, &self.context))
            {
                stack.remove(position);
            }
        });
        self.context.value.entered.store(false, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// The ambient binding
// ---------------------------------------------------------------------------

struct ThreadRoot(WriteContext);

impl ThreadRoot {
    fn new() -> Self {
        let context = WriteContext::new();
        context.value.entered.store(true, Ordering::Relaxed);
        Self(context)
    }
}

impl Drop for ThreadRoot {
    fn drop(&mut self) {
        self.0.value.entered.store(false, Ordering::Release);
    }
}

thread_local! {
    /// The implicit context is already entered for the thread's lifetime.
    static ROOT: ThreadRoot = ThreadRoot::new();
    /// Explicitly entered contexts, innermost last.
    static STACK: RefCell<Vec<WriteContext>> = const { RefCell::new(Vec::new()) };
}

/// The context that would currently observe a write.
pub fn current_context() -> WriteContext {
    STACK
        .with(|stack| stack.borrow().last().cloned())
        .unwrap_or_else(|| ROOT.with(|root| root.0.clone()))
}

/// Whether the current context would hear a write notification.
pub fn observing_writes() -> bool {
    current_context().is_observing()
}

// ---------------------------------------------------------------------------
// Install / stop / notify
// ---------------------------------------------------------------------------

/// Opaque, single-use reset token for [`observe_writes`] (Source `ContextVar.Token`).
///
/// It carries the real context identity and the previous observer value; there
/// is nothing to construct or forge outside this module.
pub struct WriteObserverToken {
    context: WriteContext,
    previous: Option<Chain>,
    used: AtomicBool,
}

impl WriteObserverToken {
    /// Whether [`stop_observing_writes`] has already consumed this token.
    pub fn is_used(&self) -> bool {
        self.used.load(Ordering::Acquire)
    }
}

/// Install `observer` in the current context (Source `observe_writes`).
///
/// An already-installed observer keeps hearing every write: the new chain calls
/// the captured outer chain first, then `observer`. If the outer chain raises,
/// the new observer is not called. Nothing is written to the filesystem.
pub fn observe_writes<F>(observer: F) -> WriteObserverToken
where
    F: Fn(&Path, &[u8]) -> Result<(), WriteObserverError> + Send + Sync + 'static,
{
    observe_writes_callback(Arc::new(observer))
}

/// [`observe_writes`] for a pre-built callback object (shared callback state).
pub fn observe_writes_callback(observer: ObserverCallback) -> WriteObserverToken {
    let context = current_context();
    let mut guard = lock(&context.value.chain);
    let previous = guard.clone();
    let next: Chain = match &previous {
        None => Arc::new(vec![observer]),
        Some(outer) => {
            let mut chain = Vec::with_capacity(outer.len() + 1);
            chain.extend(outer.iter().cloned());
            chain.push(observer);
            Arc::new(chain)
        }
    };
    *guard = Some(next);
    drop(guard);
    WriteObserverToken {
        context,
        previous,
        used: AtomicBool::new(false),
    }
}

/// Reset the observer [`observe_writes`] installed (Source `stop_observing_writes`).
///
/// The token must belong to the current context and may be consumed exactly
/// once. Resetting restores the value captured at install time — the reference
/// permits out-of-order resets, so an outer token may be reset before an inner
/// one (and the inner reset then re-installs the outer chain).
pub fn stop_observing_writes(token: &WriteObserverToken) -> Result<(), WriteContextError> {
    if token.used.load(Ordering::Acquire) {
        return Err(WriteContextError::TokenAlreadyUsed);
    }
    let context = current_context();
    if !same_context(&token.context, &context) {
        return Err(WriteContextError::ForeignContext);
    }
    if token.used.swap(true, Ordering::AcqRel) {
        return Err(WriteContextError::TokenAlreadyUsed);
    }
    context.value.replace(token.previous.clone());
    Ok(())
}

/// Tell the installed observer, if any, that `path` now holds `bytes`
/// (Source `notify_written`).
///
/// The path is passed through exactly as supplied: it may be relative, absent,
/// or hold non-UTF-8/NUL bytes, and this function performs no filesystem I/O.
/// `Ok(true)` means a chain was installed and was invoked (completely, or up to
/// the observer that raised); `Ok(false)` means there was nothing to notify.
pub fn notify_written(path: &Path, bytes: &[u8]) -> Result<bool, WriteObserverError> {
    let chain = current_context().value.snapshot();
    let Some(chain) = chain else {
        return Ok(false);
    };
    for observer in chain.iter() {
        observer(path, bytes)?;
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Future binding
// ---------------------------------------------------------------------------

/// A future bound to an explicit write context.
///
/// Created by [`bind_write_context`]; see its documentation.
pub struct BoundWriteContext<F> {
    context: WriteContext,
    future: Pin<Box<F>>,
}

impl<F> Unpin for BoundWriteContext<F> {}

impl<F: Future> Future for BoundWriteContext<F> {
    type Output = Result<F::Output, WriteContextError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let guard = match this.context.enter() {
            Ok(guard) => guard,
            Err(error) => return Poll::Ready(Err(error)),
        };
        let polled = this.future.as_mut().poll(cx);
        drop(guard);
        polled.map(Ok)
    }
}

/// Bind `future` to a snapshot of `context` taken now (task creation).
///
/// Every `poll` runs inside the snapshot (which is a distinct context, so two
/// bound futures never share a value) and the previous ambient binding is
/// restored afterwards, on return, error or unwind. Observers installed after
/// this call are not visible to the future — the snapshot is the Source
/// "context is copied when the task is created" rule, not TLS leakage.
/// The output is `Result<F::Output, WriteContextError>`: an externally retained
/// context handle that is concurrently entered yields the typed entry failure,
/// rather than allowing a second binding or panicking.
pub fn bind_write_context<F>(context: &WriteContext, future: F) -> BoundWriteContext<F> {
    BoundWriteContext {
        context: context.capture(),
        future: Box::pin(future),
    }
}

/// Poison-tolerant lock: an observer panic must not turn the context into a
/// permanently unusable value.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
