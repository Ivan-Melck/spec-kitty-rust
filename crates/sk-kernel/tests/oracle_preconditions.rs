//! A missing oracle must fail, never count as a passing differential comparison.

#[path = "support/oracle.rs"]
mod oracle_support;

#[test]
fn missing_oracle_is_a_failing_precondition() {
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "oracle_selection_child",
            "--nocapture",
        ])
        .env("SK_KERNEL_PYTHON", temp.path().join("missing-python"))
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "missing oracle was silently certified"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("required Python oracle is missing"));
}

#[test]
#[ignore = "negative subprocess helper exercised by the parent"]
fn oracle_selection_child() {
    let _ = oracle_support::python();
}
