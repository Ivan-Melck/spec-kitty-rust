//! Acceptance tests: the shared per-run cursor lock, including real
//! multi-process contention and a cross-process timeout probe.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sk_run_store::{AdvanceRequest, FileLock, RunStore, StepResult};

use support::{Fixture, MISSION_SLUG, two_step_yaml};

const CHILD_ENV: &str = "SK_RUN_STORE_CHILD";

/// The child entry point. `cargo test` runs it only when a parent sets the
/// environment variable and filters for this exact test name.
#[test]
fn child_advance() {
    let Ok(spec) = std::env::var(CHILD_ENV) else {
        return;
    };
    let fields: Vec<&str> = spec.split('|').collect();
    assert_eq!(
        fields.len(),
        4,
        "child spec: root|run_id|timeout_ms|expected"
    );
    let root = PathBuf::from(fields[0]);
    let run_id = fields[1];
    let timeout_ms: u64 = fields[2].parse().expect("timeout");
    let expected = if fields[3] == "-" {
        None
    } else {
        Some(fields[3].to_string())
    };

    let store = RunStore::open(&root)
        .expect("child store")
        .with_lock_timeout(Duration::from_millis(timeout_ms));
    let run = store
        .run_for_mission(MISSION_SLUG, None)
        .expect("child index")
        .expect("child run");
    assert_eq!(run.run_id, run_id);

    match store.advance(
        &run,
        &AdvanceRequest {
            agent_id: "child".to_string(),
            result: StepResult::Success,
            expected_issued_step: expected,
        },
    ) {
        Ok(outcome) => {
            println!(
                "CHILD_RESULT|OK|{}",
                outcome.decision.step_id().unwrap_or("-")
            );
        }
        Err(error) => println!("CHILD_RESULT|ERR|{}", error.code()),
    }
}

fn spawn_child(root: &Path, run_id: &str, timeout_ms: u64, expected: &str) -> std::process::Output {
    let exe = std::env::current_exe().expect("test binary");
    let spec = format!(
        "{}|{}|{}|{}",
        root.to_string_lossy(),
        run_id,
        timeout_ms,
        expected
    );
    Command::new(exe)
        .arg("child_advance")
        .arg("--exact")
        .arg("--nocapture")
        .env(CHILD_ENV, spec)
        .output()
        .expect("spawn child")
}

fn child_results(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("CHILD_RESULT|").map(str::to_string))
        .collect()
}

#[test]
fn a_held_cursor_lock_times_out_a_real_second_process() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;

    let lock_path = fixture.store.cursor_lock_path(run);
    let held = FileLock::acquire(&lock_path, Duration::from_secs(5)).expect("parent lock");
    assert_eq!(held.path(), lock_path.as_path());

    let output = spawn_child(&fixture.root, &run.run_id, 300, "-");
    let results = child_results(&output);
    assert_eq!(
        results,
        ["ERR|LOCK_TIMEOUT"],
        "child stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fixture.store.read_state(run).unwrap().issued_step_id,
        None,
        "the timed-out child wrote nothing"
    );

    drop(held);
    let outcome = fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "agent-a".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect("lock is free again");
    assert_eq!(outcome.decision.step_id(), Some("specify"));
}

#[test]
fn competing_processes_produce_exactly_one_advance() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;

    // First issue the step, then race six real processes to complete it.
    fixture
        .store
        .advance(
            run,
            &AdvanceRequest {
                agent_id: "seed".to_string(),
                result: StepResult::Success,
                expected_issued_step: None,
            },
        )
        .expect("seed issue");
    let before_completed = support::event_types(&run.run_dir)
        .iter()
        .filter(|event| event.as_str() == "NextStepAutoCompleted")
        .count();
    assert_eq!(before_completed, 0);

    let mut children = Vec::new();
    for _ in 0..6 {
        children.push(spawn_child(&fixture.root, &run.run_id, 5_000, "specify"));
    }

    let mut ok = 0usize;
    let mut stale = 0usize;
    for output in &children {
        let results = child_results(output);
        assert_eq!(results.len(), 1, "child output: {:?}", results);
        match results[0].as_str() {
            "OK|plan" => ok += 1,
            "ERR|STALE_ADVANCE_PLAN" => stale += 1,
            other => panic!("unexpected child result {other:?}"),
        }
    }
    assert_eq!(ok, 1, "exactly one competing advance may win");
    assert_eq!(stale, 5, "the losers must be typed stale-plan refusals");

    let state = fixture.store.read_state(run).expect("state");
    assert_eq!(state.completed_steps, ["specify"]);
    assert_eq!(state.issued_step_id.as_deref(), Some("plan"));
    let types = support::event_types(&run.run_dir);
    assert_eq!(
        types
            .iter()
            .filter(|event| event.as_str() == "NextStepAutoCompleted")
            .count(),
        1,
        "no duplicate completion event: {types:?}"
    );
    assert_eq!(
        types
            .iter()
            .filter(|event| event.as_str() == "NextStepIssued")
            .count(),
        2,
        "one issue for specify plus one for plan: {types:?}"
    );
}

#[test]
fn the_lock_sidecar_is_dedicated_and_never_the_state_payload() {
    let fixture = Fixture::with_template_yaml(two_step_yaml());
    let started = fixture.start();
    let run = &started.run;
    let lock_path = fixture.store.cursor_lock_path(run);
    assert_eq!(
        lock_path.file_name().and_then(|name| name.to_str()),
        Some("state.json.lock")
    );
    assert_ne!(lock_path, run.run_dir.join("state.json"));
    let lock = FileLock::acquire(&lock_path, Duration::from_secs(1)).expect("lock");
    assert!(run.run_dir.join("state.json").is_file());
    drop(lock);
    assert_eq!(
        std::fs::metadata(&lock_path).expect("sidecar").len(),
        0,
        "release truncates but never unlinks the lock inode"
    );
}
