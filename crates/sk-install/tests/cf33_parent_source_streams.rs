//! Source exists/read ordering and controlled streams; Native functional controls.
use sk_install::session::{
    ClaudeSettingsShape, OrientationSectionState, assess_orientation, observe_claude_settings,
};
use std::{fs, path::Path};

#[test]
fn source_absent_non_directory_component_stays_absent_before_settings_read() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join(".claude"), b"not a directory").unwrap();
    assert_eq!(
        observe_claude_settings(temp.path()).shape,
        ClaudeSettingsShape::Absent
    );
    assert!(matches!(
        assess_orientation(&temp.path().join(".claude/AGENTS.md"), "v").state,
        OrientationSectionState::FileAbsent
    ));
}

#[test]
#[cfg(unix)]
fn source_symlink_loops_are_absent_at_the_exists_phase() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".claude")).unwrap();
    symlink("settings.json", temp.path().join(".claude/settings.json")).unwrap();
    symlink("AGENTS.md", temp.path().join("AGENTS.md")).unwrap();
    assert_eq!(
        observe_claude_settings(temp.path()).shape,
        ClaudeSettingsShape::Absent
    );
    assert!(matches!(
        assess_orientation(&temp.path().join("AGENTS.md"), "v").state,
        OrientationSectionState::FileAbsent
    ));
}

#[test]
#[cfg(unix)]
fn source_orientation_stat_failure_escapes_instead_of_becoming_missing() {
    use sk_install::tool_surface_diagnostics::{
        SessionSurfacePhase, SurfaceAuditRequest, SurfaceKind, ToolSurfaceAuditFailure,
        audit_tool_surfaces,
    };
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let denied = temp.path().join("denied");
    fs::create_dir(&denied).unwrap();
    fs::write(denied.join("AGENTS.md"), b"content").unwrap();
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o0)).unwrap();
    let can_inspect = fs::metadata(denied.join("AGENTS.md")).is_ok();
    if can_inspect {
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o700)).unwrap();
        panic!("this EACCES control requires an unprivileged test process");
    }
    let result = audit_tool_surfaces(&SurfaceAuditRequest {
        project_root: &denied,
        configured_tools: &["codex".to_owned()],
        tool_filter: None,
        kinds: Some(&[SurfaceKind::ContextFile]),
    });
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o700)).unwrap();
    match result {
        Err(ToolSurfaceAuditFailure::Session(failure)) => {
            assert_eq!(failure.phase(), SessionSurfacePhase::Read)
        }
        other => panic!("stat failure must escape before read, not a missing finding: {other:?}"),
    }
}

#[test]
#[cfg(unix)]
fn source_stream_read_does_not_reject_controlled_fifo_destinations() {
    fn stream(path: &Path, bytes: &'static [u8], read: impl FnOnce()) {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
        let writer_path = path.to_path_buf();
        let writer = std::thread::spawn(move || {
            fs::write(writer_path, bytes).expect("controlled FIFO feed")
        });
        read();
        writer
            .join()
            .expect("writer completed after reader consumed EOF");
    }
    let temp = tempfile::tempdir().unwrap();
    let orientation = temp.path().join("AGENTS.md");
    stream(&orientation, b"authored stream without a section", || {
        assert!(matches!(
            assess_orientation(&orientation, "v").state,
            OrientationSectionState::NoSection
        ));
    });
    fs::create_dir(temp.path().join(".claude")).unwrap();
    let settings = temp.path().join(".claude/settings.json");
    stream(&settings, b"{}", || {
        assert_eq!(
            observe_claude_settings(temp.path()).shape,
            ClaudeSettingsShape::Object
        );
    });
}
