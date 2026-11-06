//! Real cross-process lock contention between the canonical Python writer and
//! this native writer.
//!
//! Both sides take the same `flock(2)` lock file from the pinned interim
//! event-log protocol (`<git-common-dir>/spec-kitty-locks/event-logs/<digest>.lock`,
//! or `<anchor>.event-log.lock` outside a repository). These tests start real
//! Python and real native writers, so the contention is observed between two
//! processes, not simulated inside one.
//!
//! The Python side drives `kernel.event_log_append` directly with a bounded
//! `timeout_s`, because `InvocationWriter` itself hardcodes the 10 s default
//! and a 10 s test is not useful; the primitive under test is the same one the
//! writer calls.

mod support;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use sk_invocation::{InvocationWriter, append_op_closure};
use support::{completed, python, run_python, started};

const ID: &str = "01KPQRX2EVGMRVB4Q1JQBAZJV3";
const FAMILY_LOCK_TIMEOUT: Duration = Duration::from_millis(250);

const PYTHON_BOUNDED_APPEND: &str = r#"
import json, sys
from pathlib import Path
from kernel.event_log_append import EventLogLockTimeoutError, append_event_log_lines

payload = json.loads(sys.stdin.readline())
try:
    append_event_log_lines(
        Path(payload["path"]),
        [payload["line"]],
        lock_anchor=Path(payload["anchor"]),
        create=False,
        no_follow=True,
        timeout_s=payload["timeout_s"],
    )
except EventLogLockTimeoutError:
    print("EVENT_LOG_LOCK_TIMEOUT")
else:
    print("APPENDED")
"#;

const PYTHON_HOLD_FAMILY_LOCK: &str = r#"
import json, sys
from pathlib import Path
from kernel.event_log_append import event_log_lock

payload = json.loads(sys.stdin.readline())
with event_log_lock(Path(payload["anchor"])):
    print("READY", flush=True)
    sys.stdin.readline()
"#;

fn family_lock_path(anchor: &std::path::Path) -> std::path::PathBuf {
    sk_kernel::event_log_append::event_log_lock_path(anchor).expect("family lock path")
}

#[test]
fn a_native_family_lock_blocks_the_canonical_python_append() {
    let temp = tempfile::tempdir().expect("isolated project");
    let writer = InvocationWriter::new(temp.path());
    let trail = writer.write_started(&started(ID)).expect("started").path;
    let before = fs::read(&trail).expect("read trail");
    let anchor = temp.path().join("kitty-ops");

    let guard = sk_kernel::FileLock::acquire(&family_lock_path(&anchor), Duration::from_secs(2))
        .expect("take the family lock");
    let request = serde_json::json!({
        "path": trail.to_string_lossy(),
        "anchor": anchor.to_string_lossy(),
        "line": completed(ID).to_jsonl_line(),
        "timeout_s": 0.3,
    });
    let outcome = run_python(PYTHON_BOUNDED_APPEND, Some(&format!("{request}\n")));
    assert_eq!(outcome, "EVENT_LOG_LOCK_TIMEOUT");
    assert_eq!(fs::read(&trail).expect("read trail"), before);

    drop(guard);
    let after_release = run_python(PYTHON_BOUNDED_APPEND, Some(&format!("{request}\n")));
    assert_eq!(after_release, "APPENDED");
    let lines: Vec<String> = fs::read_to_string(&trail)
        .expect("read trail")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1], completed(ID).to_jsonl_line());
}

#[test]
fn a_python_family_lock_blocks_the_native_append_and_the_native_start() {
    let temp = tempfile::tempdir().expect("isolated project");
    let writer = InvocationWriter::new(temp.path()).with_lock_timeout(FAMILY_LOCK_TIMEOUT);
    let trail = writer.write_started(&started(ID)).expect("started").path;
    let before = fs::read(&trail).expect("read trail");
    let anchor = temp.path().join("kitty-ops");
    let new_id = "01KPQRX2EVGMRVB4Q1JQBAZJV4";

    let mut child = Command::new(python())
        .arg("-c")
        .arg(PYTHON_HOLD_FAMILY_LOCK)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the lock holder");
    {
        let stdin = child.stdin.as_mut().expect("piped stdin");
        let request = serde_json::json!({"anchor": anchor.to_string_lossy()});
        writeln!(stdin, "{request}").expect("write holder request");
        stdin.flush().expect("flush holder request");
    }
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut ready = String::new();
    stdout.read_line(&mut ready).expect("read holder readiness");
    assert_eq!(ready.trim(), "READY", "the Python holder did not start");

    let append_error = writer
        .write_completed(&completed(ID))
        .expect_err("the Python process holds the family lock");
    let append_message = append_error.to_string();
    assert!(
        append_message.starts_with("Failed to append completed event: "),
        "{append_message}"
    );
    assert!(
        append_message.contains("Error code: EVENT_LOG_LOCK_TIMEOUT."),
        "{append_message}"
    );
    assert_eq!(fs::read(&trail).expect("read trail"), before);

    let start_error = writer
        .write_started(&started(new_id))
        .expect_err("the Python process holds the family lock");
    let start_message = start_error.to_string();
    assert!(
        start_message.starts_with("Failed to write invocation record: "),
        "{start_message}"
    );
    assert!(
        start_message.contains("Error code: EVENT_LOG_LOCK_TIMEOUT."),
        "{start_message}"
    );
    assert!(
        !anchor.join(format!("{new_id}.jsonl")).exists(),
        "a refused start must not create a trail"
    );

    // Releasing the Python holder restores the native writer.
    child.stdin.take();
    let status = child.wait().expect("wait for holder");
    assert!(status.success(), "the Python holder exited with {status}");
    writer
        .write_completed(&completed(ID))
        .expect("native append after release");
    assert_eq!(
        fs::read_to_string(&trail)
            .expect("read trail")
            .lines()
            .count(),
        2
    );
}

#[test]
fn a_native_thread_holding_the_family_lock_blocks_another_native_writer() {
    let temp = tempfile::tempdir().expect("isolated project");
    let writer = InvocationWriter::new(temp.path()).with_lock_timeout(FAMILY_LOCK_TIMEOUT);
    let trail = writer.write_started(&started(ID)).expect("started").path;
    let before = fs::read(&trail).expect("read trail");
    let anchor = temp.path().join("kitty-ops");

    let (acquired_sender, acquired_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let guard =
            sk_kernel::FileLock::acquire(&family_lock_path(&anchor), Duration::from_secs(2))
                .expect("take the family lock");
        acquired_sender.send(()).expect("signal acquisition");
        release_receiver.recv().expect("wait for release");
        drop(guard);
    });
    acquired_receiver
        .recv()
        .expect("the holder acquired the lock");

    let error = writer
        .write_completed(&completed(ID))
        .expect_err("the sibling thread holds the family lock");
    assert!(
        error.to_string().contains("EVENT_LOG_LOCK_TIMEOUT"),
        "{error}"
    );
    assert_eq!(fs::read(&trail).expect("read trail"), before);

    release_sender.send(()).expect("release the holder");
    holder.join().expect("holder thread");
    writer.write_completed(&completed(ID)).expect("append");
}

#[test]
fn the_closure_spine_uses_the_same_family_protocol_independently_of_the_trail() {
    // The spine's lock is anchored on the spine file (not the `kitty-ops`
    // family), so holding the family lock does not block a spine append. This
    // pins the reference's two distinct anchoring choices.
    let temp = tempfile::tempdir().expect("isolated project");
    let anchor = temp.path().join("kitty-ops");
    fs::create_dir_all(&anchor).expect("create kitty-ops");
    let guard = sk_kernel::FileLock::acquire(&family_lock_path(&anchor), Duration::from_secs(2))
        .expect("take the family lock");

    let sweep = support::completed_with(ID, |fields| {
        fields.closed_by = sk_invocation::ClosedBy::DoctorSweep;
        fields.outcome = sk_invocation::OpOutcome::Abandoned;
    });
    let path = append_op_closure(temp.path(), &sweep).expect("spine append");
    drop(guard);
    assert_eq!(
        fs::read_to_string(&path)
            .expect("read spine")
            .lines()
            .count(),
        1
    );
}
