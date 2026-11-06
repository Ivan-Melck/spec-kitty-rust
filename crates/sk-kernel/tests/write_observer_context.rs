#![cfg(unix)]
//! Source-first proof for the one canonical ambient write-observer context.
//!
//! The real pinned `kernel.atomic` observer protocol and the notifying /
//! not-notifying writers run FIRST in an actual isolated child (exact CPython
//! 3.11.15 / exact-53 lock-exact oracle, asserted roots, raw stdout/stderr kept).
//! Only after that child succeeds does the equivalent Native child run under the
//! same isolated roots and compare the portable whole outcome.
//!
//! Deliberate limit: Source reset-token messages embed real heap addresses and
//! the `ContextVar` repr, so message prose is asserted only for the intended
//! cause/prefix and is NOT compared cross-process. The Native exception wire
//! format remains OPEN; class and phase are what this test binds.

#[path = "support/oracle.rs"]
mod oracle;

use std::cell::Cell;
use std::fs;
use std::future::Future;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use serde_json::{Value, json};
use sk_kernel::source_writer::write_observer::{
    WriteContext, WriteContextError, WriteObserverError, bind_write_context, notify_written,
    observe_writes, observing_writes, stop_observing_writes,
};
use sk_kernel::source_writer::{SourcePayload, WriteObservation, WriterOperation, write_source};

#[test]
fn bound_poll_preserves_context_entry_exclusivity() {
    let context = WriteContext::new();
    let mut bound = Box::pin(bind_write_context(&context, async {
        let active = sk_kernel::source_writer::write_observer::current_context();
        assert!(
            matches!(active.enter(), Err(WriteContextError::ContextBusy)),
            "a context actively polling must not permit a second entry"
        );
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(bound.as_mut().poll(&mut cx), Poll::Ready(Ok(()))));
}

#[test]
fn bound_poll_returns_busy_for_an_escaped_entered_context() {
    let context = WriteContext::new();
    let captured = Arc::new(Mutex::new(None));
    let saved = captured.clone();
    let ready = Rc::new(Cell::new(false));
    let mut bound = Box::pin(bind_write_context(&context, async move {
        *saved.lock().unwrap() = Some(sk_kernel::source_writer::write_observer::current_context());
        Gate { ready }.await;
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(bound.as_mut().poll(&mut cx).is_pending());
    let escaped = captured.lock().unwrap().take().unwrap();
    let guard = escaped.enter().unwrap();
    assert!(matches!(
        bound.as_mut().poll(&mut cx),
        Poll::Ready(Err(WriteContextError::ContextBusy))
    ));
    drop(guard);
}

#[test]
fn guard_drop_never_pops_a_different_live_binding() {
    let outer = WriteContext::new();
    let inner = WriteContext::new();
    let outer_guard = outer.enter().unwrap();
    let inner_guard = inner.enter().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let token = observe_writes(move |_, bytes| {
        sink.lock().unwrap().push(bytes.to_vec());
        Ok(())
    });
    drop(outer_guard);
    notify_written(Path::new("inner"), b"still-inner").unwrap();
    assert_eq!(*seen.lock().unwrap(), [b"still-inner".to_vec()]);
    stop_observing_writes(&token).unwrap();
    drop(inner_guard);
    let root = sk_kernel::source_writer::write_observer::current_context();
    assert!(
        matches!(root.enter(), Err(WriteContextError::ContextBusy)),
        "the implicit thread context is already entered"
    );
}

const TEST_NAME: &str = "ambient_write_observer_matches_source_semantics";
const ENV_ROOTS: [(&str, &str); 11] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];

fn command(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("SK_WRITE_OBSERVER_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in ENV_ROOTS {
        command.env(key, root.join(name));
    }
    command
}

fn assert_native_environment(root: &Path) {
    assert_eq!(
        std::env::current_dir().expect("cwd"),
        root,
        "Native child must run in the isolated evidence root"
    );
    for (key, name) in ENV_ROOTS {
        assert_eq!(
            PathBuf::from(std::env::var_os(key).expect("root variable")),
            root.join(name),
            "{key} must point into the isolated evidence root"
        );
    }
    assert_eq!(
        PathBuf::from(std::env::var_os("PYTHONPATH").expect("PYTHONPATH")),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo")
            .join("src"),
        "Source PYTHONPATH must stay absolute"
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("SPEC_KITTY_PACKS_ROOT").expect("packs")),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
            .join("packs"),
        "packs root must be the exact pinned resource root"
    );
}

#[test]
fn ambient_write_observer_matches_source_semantics() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root");

    if let Some(root) = std::env::var_os("SK_WRITE_OBSERVER_NATIVE") {
        let root = PathBuf::from(root);
        assert_native_environment(&root);
        let source: Value = serde_json::from_slice(
            &fs::read(root.join("source.stdout")).expect("retained Source stdout"),
        )
        .expect("Source JSON document");
        let native = native_document(&root);
        fs::write(
            root.join("native.stdout.json"),
            serde_json::to_vec_pretty(&native).expect("native JSON"),
        )
        .expect("retain Native JSON");
        assert_eq!(
            portable(&native),
            portable(&source),
            "whole portable Source-first outcome"
        );
        return;
    }

    let root = tempfile::tempdir().expect("evidence root").keep();
    for (_, name) in ENV_ROOTS {
        fs::create_dir_all(root.join(name)).expect("isolated root");
    }
    eprintln!("write-observer evidence: {}", root.display());

    // Source FIRST: the pinned oracle runs the real `kernel.atomic` protocol.
    let source = command(&root, &repo, oracle::python())
        .arg(repo.join("crates/sk-kernel/tests/oracle/write_observer_context.py"))
        .output()
        .expect("execute Source oracle child");
    fs::write(root.join("source.stdout"), &source.stdout).expect("retain Source stdout");
    fs::write(root.join("source.stderr"), &source.stderr).expect("retain Source stderr");
    fs::write(
        root.join("source.status"),
        format!("{:?}", source.status.code()),
    )
    .unwrap();
    assert!(
        source.status.success(),
        "Source FIRST failed: {} {}",
        String::from_utf8_lossy(&source.stdout),
        String::from_utf8_lossy(&source.stderr)
    );
    let document: Value =
        serde_json::from_slice(&source.stdout).expect("Source oracle printed JSON");
    assert_source_causes(&document);

    // Then the equivalent Native child, under the same isolated roots.
    let native = command(&root, &repo, std::env::current_exe().expect("test binary"))
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env("SK_WRITE_OBSERVER_NATIVE", &root)
        .output()
        .expect("execute Native child");
    fs::write(root.join("native.stdout"), &native.stdout).expect("retain Native stdout");
    fs::write(root.join("native.stderr"), &native.stderr).expect("retain Native stderr");
    fs::write(
        root.join("native.status"),
        format!("{:?}", native.status.code()),
    )
    .unwrap();
    assert!(
        native.status.success(),
        "Native child failed: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}

/// The actual intended Source causes, asserted before any Native execution.
fn assert_source_causes(document: &Value) {
    assert_eq!(document["no_observer"]["no_observer_noop"], json!(true));
    assert_eq!(document["no_observer"]["install_stop_no_fs"], json!(true));
    assert_eq!(
        document["direct_notify"]["paths"],
        json!([
            ["72656c61746976652f2e2e2f7765697264", [255, 0, 254]],
            ["6e756c00696e73696465", []],
            ["6e6f6eff75746638", [98, 121, 116, 101, 115]],
        ])
    );
    assert_eq!(document["chain"]["error_class"], json!("ValueError"));
    assert_eq!(
        document["chain"]["error_message"],
        json!("observer refused")
    );
    assert_eq!(document["out_of_order"]["order"], json!(["outer"]));
    assert_eq!(document["out_of_order"]["resurrected"], json!(true));
    assert_eq!(
        document["token_classes"]["reused_class"],
        json!("RuntimeError")
    );
    assert!(
        document["token_classes"]["reused_message"]
            .as_str()
            .is_some_and(|message| message.contains("has already been used once")),
        "actual Source reused-token cause: {}",
        document["token_classes"]["reused_message"]
    );
    assert_eq!(
        document["token_classes"]["foreign_class"],
        json!("ValueError")
    );
    assert!(
        document["token_classes"]["foreign_message"]
            .as_str()
            .is_some_and(|message| message.contains("was created in a different Context")),
        "actual Source foreign-token cause: {}",
        document["token_classes"]["foreign_message"]
    );
    assert_eq!(
        document["token_classes"]["foreign_token_unused_before_reset"],
        json!(true)
    );
    assert_eq!(
        document["context_entry"]["recursive_class"],
        json!("RuntimeError")
    );
    assert_eq!(
        document["context_entry"]["concurrent_class"],
        json!("RuntimeError")
    );
    for key in ["recursive_prefix", "concurrent_prefix"] {
        assert_eq!(
            document["context_entry"][key],
            json!("cannot enter context")
        );
    }
    assert_eq!(
        document["copy"]["copy_order"],
        json!(["outer", "copy-only", "outer"])
    );
    assert_eq!(
        document["copy"]["original_after_copy_install"],
        json!(["outer"])
    );
    assert_eq!(document["copy"]["shared_state"], json!(true));
    assert_eq!(document["threads"]["copied_thread_heard"], json!(true));
    assert_eq!(document["asyncio"]["task_a"], json!(["a"]));
    assert_eq!(document["asyncio"]["task_b"], json!(["b"]));
    assert_eq!(
        document["writers"]["atomic_error_class"],
        json!("ValueError")
    );
    assert_eq!(
        document["writers"]["derived_json_bytes"],
        json!(b"{\n  \"a\": 2,\n  \"b\": 1\n}\n".to_vec())
    );
    assert_eq!(
        document["writers"]["not_notifying_after_atomic_only"],
        json!([["atomic2.txt", [120]]])
    );
    assert_eq!(document["writers"]["outside_ctx_no_listener"], json!(true));
}

/// Drop the cross-process-unstable prose (Python heap addresses / `ContextVar`
/// repr) and keep the contract facts both implementations can honour.
fn portable(document: &Value) -> Value {
    let mut document = document.clone();
    if let Some(object) = document
        .get_mut("token_classes")
        .and_then(Value::as_object_mut)
    {
        object.remove("reused_message");
        object.remove("foreign_message");
    }
    if let Some(object) = document
        .get_mut("context_entry")
        .and_then(Value::as_object_mut)
    {
        object.remove("recursive_prefix");
        object.remove("concurrent_prefix");
    }
    document
}

// ---------------------------------------------------------------------------
// Native child: the same scenarios expressed against the canonical context
// ---------------------------------------------------------------------------

fn native_document(root: &Path) -> Value {
    json!({
        "no_observer": native_no_observer(root),
        "direct_notify": native_direct_notify(),
        "chain": native_chain_and_error(),
        "out_of_order": native_out_of_order(),
        "token_classes": native_token_classes(),
        "context_entry": native_context_entry(),
        "copy": native_copy(),
        "threads": native_threads(),
        "asyncio": native_asyncio(),
        "writers": native_writers(root),
    })
}

/// Every scenario runs in its own explicit context, like the Source oracle's
/// fresh `contextvars.Context`.
fn with_context<T>(body: impl FnOnce() -> T) -> T {
    let context = WriteContext::new();
    let guard = context.enter().expect("fresh context entry");
    let value = body();
    drop(guard);
    value
}

fn listing(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .expect("read directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut out, "{byte:02x}").expect("hex write");
    }
    out
}

fn native_no_observer(root: &Path) -> Value {
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed = calls.clone();
    assert!(
        !notify_written(Path::new("does/not/exist.txt"), b"\x00\x01arbitrary")
            .expect("no observer")
    );
    assert!(!notify_written(Path::new("nul\0name"), b"").expect("no observer"));
    assert!(observed.lock().expect("calls").is_empty());

    let before = listing(root);
    let installed = calls.clone();
    let token = observe_writes(move |_, _| {
        installed
            .lock()
            .expect("calls")
            .push("unexpected".to_owned());
        Ok(())
    });
    stop_observing_writes(&token).expect("stop installed token");
    let after = listing(root);
    assert_eq!(before, after, "install/stop must not touch the filesystem");

    assert!(!notify_written(Path::new("still/nothing"), b"x").expect("no observer"));
    assert!(calls.lock().expect("calls").is_empty());
    json!({"no_observer_noop": true, "install_stop_no_fs": before == after})
}

fn native_direct_notify() -> Value {
    with_context(|| {
        let calls = Arc::new(Mutex::new(Vec::<(String, Vec<u8>)>::new()));
        let recorded = calls.clone();
        let token = observe_writes(move |path, bytes| {
            recorded
                .lock()
                .expect("calls")
                .push((hex(path.as_os_str().as_bytes()), bytes.to_vec()));
            Ok(())
        });
        assert!(notify_written(Path::new("relative/../weird"), b"\xff\x00\xfe").expect("observer"));
        assert!(notify_written(Path::new("nul\0inside"), b"").expect("observer"));
        assert!(
            notify_written(
                Path::new(std::ffi::OsStr::from_bytes(b"non\xffutf8")),
                b"bytes"
            )
            .expect("observer")
        );
        stop_observing_writes(&token).expect("stop token");
        let rows = calls.lock().expect("calls").clone();
        json!({
            "paths": rows
                .into_iter()
                .map(|(path, bytes)| json!([path, bytes]))
                .collect::<Vec<_>>()
        })
    })
}

fn native_chain_and_error() -> Value {
    with_context(|| {
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let push = |label: &'static str| {
            let order = order.clone();
            move |_: &Path, _: &[u8]| {
                order.lock().expect("order").push(label.to_owned());
                Ok(())
            }
        };
        let to_outer = observe_writes(push("outer"));
        let to_inner = observe_writes(push("inner"));
        assert!(notify_written(Path::new("chain"), b"1").expect("observer"));
        assert_eq!(
            *order.lock().expect("order"),
            vec!["outer".to_owned(), "inner".to_owned()]
        );
        order.lock().expect("order").clear();
        stop_observing_writes(&to_inner).expect("stop inner");
        assert!(notify_written(Path::new("chain"), b"2").expect("observer"));
        assert_eq!(*order.lock().expect("order"), vec!["outer".to_owned()]);
        stop_observing_writes(&to_outer).expect("stop outer");
        order.lock().expect("order").clear();
        assert!(!notify_written(Path::new("chain"), b"3").expect("no observer"));
        assert!(order.lock().expect("order").is_empty());

        let boom_order = order.clone();
        let to_boom = observe_writes(move |_, _| {
            boom_order.lock().expect("order").push("boom".to_owned());
            Err(WriteObserverError::value("observer refused"))
        });
        let to_later = observe_writes(push("later"));
        let error = notify_written(Path::new("chain"), b"4").expect_err("observer raised");
        assert!(error.is_value_error());
        assert_eq!(error.to_string(), "observer refused");
        assert_eq!(*order.lock().expect("order"), vec!["boom".to_owned()]);
        stop_observing_writes(&to_later).expect("stop later");
        stop_observing_writes(&to_boom).expect("stop boom");
        json!({"error_class": "ValueError", "error_message": "observer refused"})
    })
}

fn native_out_of_order() -> Value {
    with_context(|| {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let record = {
            let seen = seen.clone();
            move |_: &Path, _: &[u8]| {
                seen.lock().expect("seen").push("outer".to_owned());
                Ok(())
            }
        };
        let to_outer = observe_writes(record);
        let to_inner = observe_writes({
            let seen = seen.clone();
            move |_: &Path, _: &[u8]| {
                seen.lock().expect("seen").push("inner".to_owned());
                Ok(())
            }
        });
        assert!(notify_written(Path::new("o"), b"1").expect("observer"));
        assert_eq!(
            *seen.lock().expect("seen"),
            vec!["outer".to_owned(), "inner".to_owned()]
        );
        seen.lock().expect("seen").clear();
        stop_observing_writes(&to_outer).expect("out-of-order reset");
        assert!(!notify_written(Path::new("o"), b"2").expect("no observer"));
        assert!(seen.lock().expect("seen").is_empty());
        stop_observing_writes(&to_inner).expect("inner reset resurrects the outer chain");
        assert!(notify_written(Path::new("o"), b"3").expect("observer"));
        let order = seen.lock().expect("seen").clone();
        json!({"order": order, "resurrected": order == ["outer"]})
    })
}

fn native_token_classes() -> Value {
    with_context(|| {
        let reused = observe_writes(|_, _| Ok(()));
        stop_observing_writes(&reused).expect("first reset");
        let reused_class = match stop_observing_writes(&reused) {
            Err(WriteContextError::TokenAlreadyUsed) => "RuntimeError",
            other => panic!("expected a used-token refusal, got {other:?}"),
        };

        let foreign_context = WriteContext::new();
        let foreign_token = {
            let guard = foreign_context.enter().expect("foreign entry");
            let token = observe_writes(|_, _| Ok(()));
            drop(guard);
            token
        };
        let foreign_class = match stop_observing_writes(&foreign_token) {
            Err(WriteContextError::ForeignContext) => "ValueError",
            other => panic!("expected a foreign-token refusal, got {other:?}"),
        };
        let guard = foreign_context.enter().expect("re-enter own context");
        stop_observing_writes(&foreign_token).expect("still usable in its own context");
        drop(guard);

        json!({
            "reused_class": reused_class,
            "foreign_class": foreign_class,
            "foreign_token_unused_before_reset": true,
        })
    })
}

fn native_context_entry() -> Value {
    let context = WriteContext::new();
    let guard = context.enter().expect("first entry");
    let recursive = matches!(context.enter(), Err(WriteContextError::ContextBusy));
    drop(guard);
    assert!(recursive, "recursive entry must be refused");

    let shared = WriteContext::new();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder_context = shared.clone();
    let holder = std::thread::spawn(move || {
        let guard = holder_context.enter().expect("holder entry");
        entered_tx.send(()).expect("signal entry");
        release_rx.recv().expect("wait for release");
        drop(guard);
    });
    entered_rx.recv().expect("holder entered");
    let concurrent = matches!(shared.enter(), Err(WriteContextError::ContextBusy));
    release_tx.send(()).expect("release holder");
    holder.join().expect("holder joined");
    assert!(concurrent, "concurrent entry must be refused");
    // The flag is restored after the holder finished.
    drop(shared.enter().expect("context is not left busy"));

    json!({"recursive_class": "RuntimeError", "concurrent_class": "RuntimeError"})
}

fn native_copy() -> Value {
    let original = WriteContext::new();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));

    let guard = original.enter().expect("original entry");
    let token = observe_writes({
        let seen = seen.clone();
        move |_, _| {
            seen.lock().expect("seen").push("outer".to_owned());
            Ok(())
        }
    });
    drop(guard);

    let copy = original.capture();
    let notify = |context: &WriteContext, name: &'static str| {
        let guard = context.enter().expect("entry");
        notify_written(Path::new(name), name.as_bytes()).expect("notify");
        drop(guard);
    };

    notify(&copy, "copy");
    notify(&original, "original");
    assert_eq!(
        *seen.lock().expect("seen"),
        vec!["outer".to_owned(), "outer".to_owned()]
    );
    seen.lock().expect("seen").clear();

    let guard = copy.enter().expect("copy entry");
    let _copy_only = observe_writes({
        let seen = seen.clone();
        move |_, _| {
            seen.lock().expect("seen").push("copy-only".to_owned());
            Ok(())
        }
    });
    drop(guard);

    notify(&copy, "copy");
    notify(&original, "original");
    let copy_order = seen.lock().expect("seen").clone();
    seen.lock().expect("seen").clear();

    notify(&original, "original");
    let original_after_copy_install = seen.lock().expect("seen").clone();

    let guard = original.enter().expect("original entry");
    stop_observing_writes(&token).expect("stop original token");
    drop(guard);

    json!({
        "copy_order": copy_order,
        "original_after_copy_install": original_after_copy_install,
        "shared_state": true,
    })
}

fn native_threads() -> Value {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = WriteContext::new();
    let guard = captured.enter().expect("captured entry");
    let token = observe_writes({
        let seen = seen.clone();
        move |_, _| {
            seen.lock().expect("seen").push("captured".to_owned());
            Ok(())
        }
    });
    drop(guard);

    let plain_seen = seen.clone();
    let plain = std::thread::spawn(move || {
        let _ = notify_written(Path::new("plain"), b"plain");
        plain_seen
            .lock()
            .expect("seen")
            .push("plain-thread".to_owned());
    });
    plain.join().expect("plain thread");
    let after_plain = seen.lock().expect("seen").clone();
    assert!(
        after_plain.contains(&"plain-thread".to_owned())
            && !after_plain.contains(&"captured".to_owned()),
        "a new thread must start with no observer: {after_plain:?}"
    );
    seen.lock().expect("seen").clear();

    let copied_seen = seen.clone();
    let copied_context = captured.clone();
    let copied = std::thread::spawn(move || {
        let guard = copied_context.enter().expect("captured context on thread");
        notify_written(Path::new("t"), b"t").expect("notify captured");
        let heard = copied_seen
            .lock()
            .expect("seen")
            .contains(&"captured".to_owned());
        drop(guard);
        heard
    });
    let copied_thread_heard = copied.join().expect("copied thread");
    assert!(copied_thread_heard);

    let guard = captured.enter().expect("captured entry");
    stop_observing_writes(&token).expect("stop captured token");
    drop(guard);
    json!({"copied_thread_heard": copied_thread_heard})
}

struct Gate {
    ready: Rc<Cell<bool>>,
}

impl Future for Gate {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.ready.get() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

fn native_asyncio() -> Value {
    with_context(|| {
        let seen_a = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen_b = Arc::new(Mutex::new(Vec::<String>::new()));
        let main_seen = Arc::new(Mutex::new(Vec::<String>::new()));

        let context_a = WriteContext::new();
        let guard = context_a.enter().expect("a entry");
        let token_a = observe_writes({
            let seen = seen_a.clone();
            move |_, _| {
                seen.lock().expect("seen").push("a".to_owned());
                Ok(())
            }
        });
        drop(guard);

        let context_b = WriteContext::new();
        let guard = context_b.enter().expect("b entry");
        let token_b = observe_writes({
            let seen = seen_b.clone();
            move |_, _| {
                seen.lock().expect("seen").push("b".to_owned());
                Ok(())
            }
        });
        drop(guard);

        let ready = Rc::new(Cell::new(false));
        let gate_a = ready.clone();
        let mut future_a = Box::pin(bind_write_context(&context_a, async move {
            Gate { ready: gate_a }.await;
            notify_written(Path::new("a"), b"a").expect("notify a");
            "a"
        }));
        let gate_b = ready.clone();
        let mut future_b = Box::pin(bind_write_context(&context_b, async move {
            Gate { ready: gate_b }.await;
            notify_written(Path::new("b"), b"b").expect("notify b");
            "b"
        }));

        // Snapshot-at-task-creation: an observer installed after binding must not
        // reach either captured future.
        let ambient = main_seen.clone();
        let later = observe_writes(move |_, _| {
            ambient.lock().expect("seen").push("main".to_owned());
            Ok(())
        });

        let mut task_context = Context::from_waker(Waker::noop());
        assert!(future_a.as_mut().poll(&mut task_context).is_pending());
        assert!(future_b.as_mut().poll(&mut task_context).is_pending());
        ready.set(true);
        assert!(matches!(
            future_b.as_mut().poll(&mut task_context),
            Poll::Ready(Ok("b"))
        ));
        assert!(matches!(
            future_a.as_mut().poll(&mut task_context),
            Poll::Ready(Ok("a"))
        ));
        stop_observing_writes(&later).expect("stop later ambient");

        assert!(
            main_seen.lock().expect("seen").is_empty(),
            "no TLS leak into the surfaced ambient context"
        );
        // The bound futures left no ambient observer behind on this thread.
        assert!(!observing_writes());

        let guard = context_a.enter().expect("a entry");
        stop_observing_writes(&token_a).expect("stop a");
        drop(guard);
        let guard = context_b.enter().expect("b entry");
        stop_observing_writes(&token_b).expect("stop b");
        drop(guard);

        json!({
            "task_a": seen_a.lock().expect("seen").clone(),
            "task_b": seen_b.lock().expect("seen").clone(),
        })
    })
}

fn native_writers(root: &Path) -> Value {
    let directory = root.join("native-writers");
    fs::create_dir_all(&directory).expect("writers directory");
    let base = directory.as_path();

    let mut document = with_context(|| {
        let atomic_path = base.join("atomic.txt");
        fs::write(&atomic_path, b"old").expect("seed atomic");
        fs::set_permissions(&atomic_path, fs::Permissions::from_mode(0o444))
            .expect("seed read-only mode");

        let seen = Arc::new(Mutex::new(Vec::<(String, Vec<u8>)>::new()));
        let observed = seen.clone();
        let token = observe_writes(move |path, bytes| {
            observed.lock().expect("seen").push((
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                bytes.to_vec(),
            ));
            Ok(())
        });

        let receipt = write_source(
            WriterOperation::atomic(),
            &atomic_path,
            SourcePayload::Bytes(b"new"),
            None,
        )
        .expect("atomic write");
        assert_eq!(receipt.observation, WriteObservation::Notified);
        assert_eq!(
            *seen.lock().expect("seen"),
            vec![("atomic.txt".to_owned(), b"new".to_vec())]
        );
        assert_eq!(fs::read(&atomic_path).expect("bytes"), b"new");
        assert_eq!(
            fs::metadata(&atomic_path)
                .expect("mode")
                .permissions()
                .mode()
                & 0o777,
            0o444
        );
        seen.lock().expect("seen").clear();

        let to_boom = observe_writes(|_, _| Err(WriteObserverError::value("ledger refused")));
        let error = write_source(
            WriterOperation::atomic(),
            &atomic_path,
            SourcePayload::Bytes(b"later"),
            None,
        )
        .expect_err("raising ambient observer");
        let atomic_error_class = if error
            .observer_cause()
            .is_some_and(WriteObserverError::is_value_error)
        {
            "ValueError"
        } else {
            "other"
        };
        assert_eq!(atomic_error_class, "ValueError");
        assert_eq!(fs::read(&atomic_path).expect("bytes"), b"later");
        assert_eq!(
            fs::metadata(&atomic_path)
                .expect("mode")
                .permissions()
                .mode()
                & 0o777,
            0o444,
            "the mode restore already ran before the observer raised"
        );
        assert_eq!(
            listing(base),
            vec!["atomic.txt".to_owned()],
            "no temporary artifact survives the published write"
        );
        stop_observing_writes(&to_boom).expect("stop boom");

        let derived = {
            let value = json!({"b": 1, "a": 2});
            let mut text = serde_json::to_string_pretty(&value).expect("json");
            text.push('\n');
            text.into_bytes()
        };
        seen.lock().expect("seen").clear();
        let derived_path = base.join("status.json");
        let receipt = write_source(
            WriterOperation::DerivedJson,
            &derived_path,
            SourcePayload::Bytes(&derived),
            None,
        )
        .expect("derived write");
        assert_eq!(receipt.observation, WriteObservation::Notified);
        assert_eq!(
            *seen.lock().expect("seen"),
            vec![("status.json".to_owned(), derived.clone())]
        );

        seen.lock().expect("seen").clear();
        assert_eq!(
            write_source(
                WriterOperation::generated(false),
                &base.join("generated.txt"),
                SourcePayload::Text("generated"),
                None,
            )
            .expect("generated write")
            .observation,
            WriteObservation::NotApplicable
        );
        assert_eq!(
            write_source(
                WriterOperation::TextTruncate,
                &base.join("plain.txt"),
                SourcePayload::Text("plain"),
                None,
            )
            .expect("text write")
            .observation,
            WriteObservation::NotApplicable
        );
        assert_eq!(
            write_source(
                WriterOperation::RebuildLog,
                &base.join("events.jsonl"),
                SourcePayload::Bytes(b"[]"),
                None,
            )
            .expect("rebuild write")
            .observation,
            WriteObservation::NotApplicable
        );
        assert!(
            seen.lock().expect("seen").is_empty(),
            "Generated/TextTruncate/RebuildLog never notify"
        );

        assert_eq!(
            write_source(
                WriterOperation::atomic(),
                &base.join("atomic2.txt"),
                SourcePayload::Bytes(b"x"),
                None,
            )
            .expect("atomic2 write")
            .observation,
            WriteObservation::Notified
        );
        let not_notifying = seen.lock().expect("seen").clone();
        stop_observing_writes(&token).expect("stop token");

        json!({
            "atomic_error_class": atomic_error_class,
            "derived_json_bytes": derived,
            "not_notifying_after_atomic_only": not_notifying
                .into_iter()
                .map(|(name, bytes)| json!([name, bytes]))
                .collect::<Vec<_>>(),
        })
    });
    document.as_object_mut().expect("writers object").insert(
        "outside_ctx_no_listener".to_owned(),
        json!(!observing_writes()),
    );
    document
}

/// The typed observer-error capability (Native contract, no Source claim).
///
/// A typed `OSError` stays an `io::Error` and is `OSError`-class; a typed
/// `ValueError` is not misread as one; a message-only listener failure declares
/// no class and is never guessed into either.
#[test]
fn typed_observer_errors_keep_their_class_and_chain() {
    let os = WriteObserverError::os(io::Error::other("observer os"));
    assert!(os.is_os_error());
    assert!(!os.is_value_error());
    assert_eq!(os.io_error().expect("io error").to_string(), "observer os");

    let value = WriteObserverError::value("observer value");
    assert!(value.is_value_error());
    assert!(!value.is_os_error());
    assert!(value.io_error().is_none());

    let message = WriteObserverError::from_message("no class");
    assert!(!message.is_os_error());
    assert!(!message.is_value_error());

    let custom = WriteObserverError::custom(io::Error::other("custom"));
    assert!(!custom.is_os_error());
    assert!(std::error::Error::source(&custom).is_some());
}

// ---------------------------------------------------------------------------
// Native contract details for the same protocol (Source-grounded above)
// ---------------------------------------------------------------------------

#[test]
fn dropped_token_does_not_silently_reset_the_observer() {
    with_context(|| {
        let seen = Arc::new(Mutex::new(0usize));
        let counter = seen.clone();
        let token = observe_writes(move |_, _| {
            *counter.lock().expect("counter") += 1;
            Ok(())
        });
        // Dropping the handle is not a reset: Source keeps the observer until
        // `stop_observing_writes` is called (the token is unusable after a drop).
        drop(token);
        assert!(observing_writes());
        assert!(notify_written(Path::new("p"), b"p").expect("observer"));
        assert_eq!(*seen.lock().expect("counter"), 1);
    });
}

#[test]
fn context_change_during_notification_uses_the_snapshot_then_the_new_chain() {
    with_context(|| {
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let installed = Arc::new(Mutex::new(
            None::<sk_kernel::source_writer::WriteObserverToken>,
        ));
        let outer = {
            let order = order.clone();
            let installed = installed.clone();
            move |_: &Path, _: &[u8]| {
                order.lock().expect("order").push("outer".to_owned());
                let mut slot = installed.lock().expect("slot");
                if slot.is_none() {
                    let inner_order = order.clone();
                    *slot = Some(observe_writes(move |_: &Path, _: &[u8]| {
                        inner_order.lock().expect("order").push("inner".to_owned());
                        Ok(())
                    }));
                }
                Ok(())
            }
        };
        let outer_token = observe_writes(outer);

        assert!(notify_written(Path::new("n"), b"1").expect("observer"));
        assert_eq!(
            *order.lock().expect("order"),
            vec!["outer".to_owned()],
            "the in-flight chain is the snapshot; the new observer is not called"
        );
        assert!(notify_written(Path::new("n"), b"2").expect("observer"));
        assert_eq!(
            *order.lock().expect("order"),
            vec!["outer".to_owned(), "outer".to_owned(), "inner".to_owned()],
            "the next notification hears outer first, then the newly installed observer"
        );

        let inner_token = installed.lock().expect("slot").take();
        if let Some(inner_token) = inner_token {
            stop_observing_writes(&inner_token).expect("stop inner");
        }
        stop_observing_writes(&outer_token).expect("stop outer");
    });
}

#[test]
fn recursive_notification_reenters_without_holding_a_registry_borrow() {
    with_context(|| {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let depth = Arc::new(AtomicUsize::new(0));
        let token = observe_writes({
            let order = order.clone();
            let depth = depth.clone();
            move |_: &Path, _: &[u8]| {
                let level = depth.fetch_add(1, Ordering::SeqCst);
                order.lock().expect("order").push(format!("enter{level}"));
                if level == 0 {
                    notify_written(Path::new("nested"), b"n").expect("reentrant notification");
                }
                order.lock().expect("order").push(format!("leave{level}"));
                Ok(())
            }
        });

        assert!(notify_written(Path::new("outer"), b"o").expect("observer"));
        assert_eq!(
            *order.lock().expect("order"),
            vec![
                "enter0".to_owned(),
                "enter1".to_owned(),
                "leave1".to_owned(),
                "leave0".to_owned(),
            ],
            "the observer runs again from inside itself; no registry borrow is held"
        );
        stop_observing_writes(&token).expect("stop");
    });
}
