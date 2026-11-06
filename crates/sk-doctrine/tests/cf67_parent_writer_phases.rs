#![cfg(unix)]
use sk_doctrine::{ContextStateValue, write_context_state};
use sk_kernel::source_writer::write_observer::{
    WriteContext, WriteObserverError, observe_writes, stop_observing_writes,
};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[test]
fn equal_state_bytes_still_publish_with_source_atomic_mode() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("context-state.json");
    let state = ContextStateValue::default_state();
    fs::write(&path, state.render()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::metadata(&path).unwrap().ino();
    write_context_state(&path, &state).unwrap();
    assert_ne!(
        fs::metadata(&path).unwrap().ino(),
        before,
        "Source always replaces equal bytes"
    );
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn state_write_notifies_the_source_ambient_observer_even_for_equal_bytes() {
    let context = WriteContext::new();
    let _guard = context.enter().unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("context-state.json");
    let state = ContextStateValue::default_state();
    fs::write(&path, state.render()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observer_calls = calls.clone();
    let token = observe_writes(move |_, _| {
        observer_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let result = write_context_state(&path, &state);
    stop_observing_writes(&token).unwrap();
    result.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn state_write_observer_failure_escapes_after_publication() {
    let context = WriteContext::new();
    let _guard = context.enter().unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("context-state.json");
    fs::write(&path, "before").unwrap();
    let state = ContextStateValue::default_state();
    let token = observe_writes(|_, _| {
        Err(WriteObserverError::value(
            "parent post-publication observer",
        ))
    });
    let result = write_context_state(&path, &state);
    stop_observing_writes(&token).unwrap();
    let error = result.expect_err("Source observer error must not disappear");
    let sk_doctrine::ContextStateError::Write { source, .. } = error else {
        panic!("typed write cause expected");
    };
    assert!(source.published_before_error());
    assert!(
        source
            .observer_cause()
            .is_some_and(|cause| cause.is_value_error())
    );
    assert_eq!(fs::read(&path).unwrap(), state.render().as_bytes());
}
