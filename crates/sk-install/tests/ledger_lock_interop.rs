//! The pinned writer and native ledger must exclude each other at one path.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

fn environment(command: &mut Command, sandbox: &Path, state: Option<&str>) {
    command
        .current_dir(sandbox)
        .env("HOME", sandbox.join("home"))
        .env("USERPROFILE", sandbox.join("home"))
        .env("XDG_CONFIG_HOME", sandbox.join("config"))
        .env("XDG_CACHE_HOME", sandbox.join("other-cache"))
        .env("TMPDIR", sandbox)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONNOUSERSITE", "1");
    command.env_remove("XDG_STATE_HOME");
    if let Some(state) = state {
        command.env("XDG_STATE_HOME", state);
    }
}

#[test]
fn native_and_python_share_ledger_locks_with_unset_empty_and_relative_state_directories() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let python = oracle::python();
    for state in [None, Some(""), Some("relative-state")] {
        let sandbox = tempfile::tempdir().unwrap();
        let ledger = sandbox
            .path()
            .join("not-created/.kittify/migration-ledger.jsonl");
        let output_path = sandbox.path().join("native-path.txt");
        let mut command = Command::new(&python);
        environment(&mut command, sandbox.path(), state);
        let mut holder = command.env("PYTHONPATH", repo.join("src"))
            .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
            .args(["-c", "from pathlib import Path; import sys; from specify_cli.upgrade.ledger import _ledger_lock; p=Path(sys.argv[1]);\nwith _ledger_lock(p) as lock:\n print(str(lock), flush=True)\n sys.stdin.buffer.read(1)"])
            .arg(&ledger).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut ready = String::new();
        BufReader::new(holder.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert!(
            !ready.trim().is_empty(),
            "oracle failed before acquiring the ledger lock"
        );
        let mut native = Command::new(std::env::current_exe().unwrap());
        environment(&mut native, sandbox.path(), state);
        let output = native
            .args(["--exact", "ledger_lock_child", "--ignored", "--nocapture"])
            .env("SK_LEDGER_TEST_PATH", &ledger)
            .env("SK_LEDGER_TEST_OUTPUT", &output_path)
            .output()
            .unwrap();
        drop(holder.stdin.take());
        let oracle_result = holder.wait_with_output().unwrap();
        assert!(
            oracle_result.status.success(),
            "{}",
            String::from_utf8_lossy(&oracle_result.stderr)
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_to_string(output_path).unwrap(), ready.trim_end());
        assert!(
            !ledger.exists(),
            "path discovery and contention never create a ledger"
        );
    }
}

#[test]
#[ignore = "subprocess helper: exercised by the cross-language parent"]
fn ledger_lock_child() {
    let ledger = std::env::var_os("SK_LEDGER_TEST_PATH").expect("parent fixture");
    let lock = sk_install::ledger::ledger_lock_path(Path::new(&ledger)).unwrap();
    fs::write(
        std::env::var_os("SK_LEDGER_TEST_OUTPUT").unwrap(),
        lock.to_str().unwrap(),
    )
    .unwrap();
    assert!(
        sk_kernel::FileLock::acquire(&lock, Duration::from_millis(40)).is_err(),
        "native acquired a ledger lock already held by the pinned Python writer"
    );
}
