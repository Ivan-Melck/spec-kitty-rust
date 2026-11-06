#[path = "cf65_support/mod.rs"]
mod support;

use sk_kernel::source_writer::write_observer::{
    WriteContext, observe_writes, stop_observing_writes,
};
use std::{
    fs,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[test]
fn structured_stage_reloads_state_after_the_completed_first_mark() {
    let root = support::fixture("cf67-parent-second-read");
    assert!(
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
    let charter = root.join(".kittify/charter");
    fs::create_dir_all(&charter).unwrap();
    let body = "# Parent read probe\n\n## Policy Summary\n\n- Keep data.\n";
    fs::write(charter.join("charter.md"), body).unwrap();
    fs::write(
        charter.join("charter.yaml"),
        "schema_version: \"1.1.0\"\ncatalog:\n  references: []\n",
    )
    .unwrap();
    fs::write(
        charter.join("metadata.yaml"),
        format!("charter_hash: {}\n", sk_charter::hash_content(body)),
    )
    .unwrap();
    let state = charter.join("context-state.json");
    let observed_state = state.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = calls.clone();
    let context = WriteContext::new();
    let _guard = context.enter().unwrap();
    let token = observe_writes(move |path, _| {
        if path == observed_state {
            observed_calls.fetch_add(1, Ordering::SeqCst);
            fs::write(&observed_state, "1".repeat(4301)).unwrap();
        }
        Ok(())
    });
    let result = sk_cli::compat::charter::execute_context(
        &root,
        "implement",
        Some("software-dev"),
        None,
        false,
        true,
        true,
    );
    stop_observing_writes(&token).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "first-stage state was published and observed"
    );
    assert!(
        result.is_err(),
        "Source second-state ValueError must not become successful JSON"
    );
    assert_eq!(fs::read(&state).unwrap(), "1".repeat(4301).as_bytes());
}
