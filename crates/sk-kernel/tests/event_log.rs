//! Acceptance tests for the locked batch event-log append protocol.
//!
//! The protocol under test is documented in `src/event_log_append.rs`: one
//! exclusive `FileLock` per anchor, one `O_APPEND` unit write, fsync, readback
//! verification, and rollback to the pre-append size on any short write,
//! sync failure, or readback mismatch. Torn-tail handling is an explicit
//! per-log policy, never an implicit repair.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use sk_kernel::event_log_append::{
    EventLogAppendOptions, TornTailPolicy, append_event_log_lines, event_log_lock_path,
};
use sk_kernel::{FileLock, KernelError};
use tempfile::TempDir;

fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).expect("read")
}

fn options() -> EventLogAppendOptions {
    EventLogAppendOptions::default()
}

#[test]
fn appends_a_batch_as_one_unit_and_preserves_existing_bytes() {
    let temp = temp();
    let log = temp.path().join("decision.events.jsonl");
    let existing = b"{\"user\":\"authored\",\"comment\":\"keep me\"}\n# a user comment\n";
    fs::write(&log, existing).unwrap();

    append_event_log_lines(&log, &["{\"a\":1}", "{\"b\":2}"], &options()).expect("append");

    let mut expected = existing.to_vec();
    expected.extend_from_slice(b"{\"a\":1}\n{\"b\":2}\n");
    assert_eq!(read(&log), expected);
}

#[test]
fn creates_the_log_and_optional_parents() {
    let temp = temp();
    let log = temp.path().join("nested/logs/events.jsonl");
    let mut options = options();
    options.create_parents = true;

    append_event_log_lines(&log, &["one"], &options).expect("append");
    assert_eq!(read(&log), b"one\n");
}

#[test]
fn refuses_a_missing_parent_before_placing_a_lock() {
    let temp = temp();
    let log = temp.path().join("missing/events.jsonl");
    let error = append_event_log_lines(&log, &["one"], &options()).unwrap_err();
    assert!(matches!(error, KernelError::Io { .. }), "{error}");
    assert!(!temp.path().join("missing").exists());
}

#[test]
fn rejects_empty_and_multiline_units_without_touching_the_log() {
    let temp = temp();
    let log = temp.path().join("events.jsonl");
    fs::write(&log, b"kept\n").unwrap();

    assert!(matches!(
        append_event_log_lines(&log, &[] as &[&str], &options()),
        Err(KernelError::InvalidLogLine { .. })
    ));
    assert!(matches!(
        append_event_log_lines(&log, &["two\nlines"], &options()),
        Err(KernelError::InvalidLogLine { .. })
    ));
    assert!(matches!(
        append_event_log_lines(&log, &["carriage\rreturn"], &options()),
        Err(KernelError::InvalidLogLine { .. })
    ));
    assert_eq!(read(&log), b"kept\n");
}

#[test]
fn torn_tail_policy_refuse_writes_nothing() {
    let temp = temp();
    let log = temp.path().join("events.jsonl");
    fs::write(&log, b"complete\nhalf").unwrap();

    let error = append_event_log_lines(&log, &["next"], &options()).unwrap_err();
    assert!(matches!(error, KernelError::TornTail { .. }), "{error}");
    assert_eq!(read(&log), b"complete\nhalf");
}

#[test]
fn torn_tail_policy_rollback_cuts_to_the_last_newline_then_appends() {
    let temp = temp();
    let log = temp.path().join("events.jsonl");
    fs::write(&log, b"complete\nhalf-written").unwrap();

    let mut options = options();
    options.torn_tail = TornTailPolicy::TruncateToLastNewline;
    append_event_log_lines(&log, &["next"], &options).expect("append");

    assert_eq!(read(&log), b"complete\nnext\n");
}

#[test]
fn torn_tail_rollback_handles_a_file_without_any_newline() {
    let temp = temp();
    let log = temp.path().join("events.jsonl");
    fs::write(&log, b"no newline at all").unwrap();

    let mut options = options();
    options.torn_tail = TornTailPolicy::TruncateToLastNewline;
    append_event_log_lines(&log, &["first"], &options).expect("append");

    assert_eq!(read(&log), b"first\n");
}

#[test]
fn precondition_is_checked_under_the_lock_and_its_refusal_writes_nothing() {
    let temp = temp();
    let log = temp.path().join("events.jsonl");
    fs::write(&log, b"state\n").unwrap();

    let error = sk_kernel::event_log_append::append_event_log_lines_guarded(
        &log,
        &["next"],
        &options(),
        |existing| {
            assert_eq!(existing, b"state\n");
            Err(KernelError::GuardedRead {
                path: log.clone(),
                reason: "business refusal".to_owned(),
            })
        },
    )
    .unwrap_err();
    assert!(matches!(error, KernelError::GuardedRead { .. }));
    assert_eq!(read(&log), b"state\n");
}

#[test]
fn a_precondition_observes_the_bytes_after_an_explicit_torn_tail_repair() {
    let temp = temp();
    let log = temp.path().join("events.jsonl");
    fs::write(&log, b"good\ntorn").unwrap();
    let mut options = options();
    options.torn_tail = TornTailPolicy::TruncateToLastNewline;
    let mut observed = None;

    sk_kernel::event_log_append::append_event_log_lines_guarded(
        &log,
        &["next"],
        &options,
        |existing| {
            observed = Some(existing.to_vec());
            Ok(())
        },
    )
    .expect("append after repair");

    assert_eq!(observed.as_deref(), Some(&b"good\n"[..]));
    assert_eq!(read(&log), b"good\nnext\n");
}

#[test]
fn append_times_out_while_another_holder_owns_the_event_log_lock() {
    let temp = temp();
    let log = temp.path().join("events.jsonl");
    fs::write(&log, b"").unwrap();
    let lock_path = event_log_lock_path(&log).expect("lock path");
    let ready = std::sync::mpsc::channel();
    let release = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let held = FileLock::acquire(&lock_path, Duration::from_secs(1)).expect("hold lock");
        ready.0.send(()).expect("signal ready");
        release.1.recv().expect("wait for release");
        drop(held);
    });
    ready.1.recv().expect("holder ready");

    let mut options = options();
    options.timeout = Duration::from_millis(120);
    let error = append_event_log_lines(&log, &["next"], &options).unwrap_err();
    assert!(matches!(error, KernelError::LockTimeout { .. }), "{error}");
    assert_eq!(read(&log), b"");

    release.0.send(()).expect("release holder");
    holder.join().expect("join holder");
    append_event_log_lines(&log, &["next"], &options).expect("acquires after release");
    assert_eq!(read(&log), b"next\n");
}

#[test]
fn event_log_lock_path_uses_the_git_common_dir_locks_directory() {
    let temp = temp();
    let git = temp.path().join(".git");
    fs::create_dir(&git).unwrap();
    fs::create_dir(git.join("objects")).unwrap();
    fs::create_dir(git.join("refs")).unwrap();
    fs::write(git.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    let log = temp.path().join("kitty-specs/mission/events.jsonl");
    fs::create_dir_all(log.parent().unwrap()).unwrap();

    let lock = event_log_lock_path(&log).unwrap();
    assert_eq!(
        lock.parent().unwrap(),
        git.join("spec-kitty-locks/event-logs")
    );
    assert!(
        lock.file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".lock")
    );
}

#[test]
fn event_log_lock_path_falls_back_beside_the_log_outside_git() {
    let temp = temp();
    let log = temp.path().join("events.jsonl");
    let lock = event_log_lock_path(&log).unwrap();
    assert_eq!(lock, temp.path().join("events.jsonl.event-log.lock"));
}

#[test]
fn event_log_lock_path_resolves_a_worktree_commondir_without_running_git() {
    let temp = temp();
    let common = temp.path().join("main/.git");
    let worktree_gitdir = common.join("worktrees/lane");
    fs::create_dir_all(&worktree_gitdir).unwrap();
    fs::create_dir(common.join("objects")).unwrap();
    fs::create_dir(common.join("refs")).unwrap();
    fs::write(common.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    fs::write(worktree_gitdir.join("commondir"), b"../..\n").unwrap();
    let worktree = temp.path().join("lane");
    fs::create_dir_all(worktree.join("kitty-specs")).unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", worktree_gitdir.display()),
    )
    .unwrap();
    let log = worktree.join("kitty-specs/events.jsonl");

    let lock = event_log_lock_path(&log).unwrap();
    assert_eq!(
        lock.parent().unwrap(),
        common.join("spec-kitty-locks/event-logs")
    );
}

#[cfg(unix)]
#[test]
fn no_follow_refuses_a_symlinked_log() {
    use std::os::unix::fs::symlink;

    let temp = temp();
    let target = temp.path().join("target.jsonl");
    fs::write(&target, b"kept\n").unwrap();
    let log = temp.path().join("events.jsonl");
    symlink(&target, &log).unwrap();

    let mut options = options();
    options.no_follow = true;
    let error = append_event_log_lines(&log, &["next"], &options).unwrap_err();
    assert!(matches!(error, KernelError::NoFollow { .. }), "{error}");
    assert_eq!(read(&target), b"kept\n");
}

#[cfg(unix)]
#[test]
fn concurrent_processes_append_only_whole_lines() {
    const PROCESSES: usize = 3;
    const PER_PROCESS: usize = 12;

    let temp = temp();
    let log = temp.path().join("events.jsonl");
    fs::write(&log, b"").unwrap();
    let counter = temp.path().join("ready-count");

    let mut children = Vec::new();
    for id in 0..PROCESSES {
        let child = Command::new(std::env::current_exe().expect("current exe"))
            .args(["--exact", "event_log_child_appends", "--nocapture"])
            .env("SK_KERNEL_EVENTLOG_CHILD", &log)
            .env("SK_KERNEL_EVENTLOG_CHILD_ID", id.to_string())
            .env("SK_KERNEL_EVENTLOG_CHILD_COUNT", PER_PROCESS.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn child");
        children.push(child);
    }
    for child in &mut children {
        let status = child.wait().expect("wait");
        assert!(status.success(), "child failed: {status}");
    }

    let text = String::from_utf8(read(&log)).expect("utf8 log");
    assert!(text.ends_with('\n'));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), PROCESSES * PER_PROCESS);
    let mut next = [0_usize; PROCESSES];
    for line in lines {
        let (id, sequence) = line
            .strip_prefix("{\"writer\":")
            .and_then(|rest| rest.split_once(','))
            .expect("canonical row");
        let sequence = sequence
            .strip_prefix("\"sequence\":")
            .and_then(|rest| rest.strip_suffix('}'))
            .expect("sequence field");
        let id = id.parse::<usize>().expect("writer id");
        let sequence = sequence.parse::<usize>().expect("sequence");
        assert!(id < PROCESSES, "unknown writer in: {line}");
        assert_eq!(
            sequence, next[id],
            "writer {id} rows were reordered or lost at: {line}"
        );
        next[id] += 1;
    }
    assert_eq!(next, [PER_PROCESS; PROCESSES]);
    let _ = counter;
}

#[test]
fn event_log_child_appends() {
    let Some(log) = std::env::var_os("SK_KERNEL_EVENTLOG_CHILD") else {
        return;
    };
    let log = PathBuf::from(log);
    let id = std::env::var("SK_KERNEL_EVENTLOG_CHILD_ID").expect("writer id");
    let count = std::env::var("SK_KERNEL_EVENTLOG_CHILD_COUNT")
        .expect("count")
        .parse::<usize>()
        .expect("numeric count");
    for sequence in 0..count {
        let line = format!("{{\"writer\":{id},\"sequence\":{sequence}}}");
        append_event_log_lines(&log, &[line], &options()).expect("child append");
    }
}
