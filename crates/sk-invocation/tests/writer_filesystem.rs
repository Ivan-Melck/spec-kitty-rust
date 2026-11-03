//! Real-filesystem behaviour of [`sk_invocation::InvocationWriter`].
//!
//! Every test uses its own temporary project root. No test changes the process
//! working directory, HOME, or umask: paths are passed explicitly and the
//! owner-only mode is asserted as "no group/other permission bits", which the
//! process umask can only tighten.

mod support;

use std::fs;
use std::path::Path;

use sk_invocation::{
    ClosedBy, CorrelationLink, InvocationError, InvocationId, InvocationWriter, ModeOfWork,
    OpOutcome, RecordedMode, append_op_closure, closed_invocation_ids, normalise_ref,
    op_closures_path, read_op_closures,
};
use support::{completed, completed_with, started, started_with};

const ID: &str = "01KPQRX2EVGMRVB4Q1JQBAZJV3";
const OTHER_ID: &str = "01KPQRX2EVGMRVB4Q1JQBAZJV4";

fn trail(root: &Path) -> std::path::PathBuf {
    root.join("kitty-ops").join(format!("{ID}.jsonl"))
}

fn assert_owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(
            mode & 0o077,
            0,
            "{} must not be group/other accessible (mode {mode:o})",
            path.display()
        );
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[test]
fn started_write_creates_the_trail_index_and_events_dir() {
    let temp = tempfile::tempdir().unwrap();
    let record = started(ID);
    let writer = InvocationWriter::new(temp.path());

    let outcome = writer.write_started(&record).expect("started write");

    let expected = temp.path().join("kitty-ops").join(format!("{ID}.jsonl"));
    assert_eq!(outcome.path, expected);
    assert_eq!(
        fs::read_to_string(&outcome.path).unwrap(),
        format!("{}\n", record.to_jsonl_line())
    );
    assert_owner_only(&outcome.path);

    let index = temp.path().join("kitty-ops/ops-index.jsonl");
    let index_text = fs::read_to_string(&index).unwrap();
    assert_eq!(
        index_text,
        format!(
            "{{\"invocation_id\": \"{ID}\", \"profile_id\": \"implementer-fixture\", \"started_at\": \"2026-04-21T10:00:00+00:00\"}}\n"
        )
    );
    assert_owner_only(&index);
    assert_eq!(outcome.index_warning, None);
    assert!(!temp.path().join("invocation-index.jsonl").exists());
    assert!(!temp.path().join(".kittify").exists());
}

#[test]
fn a_second_start_for_the_same_id_is_a_collision() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    writer.write_started(&started(ID)).expect("first write");

    let error = writer.write_started(&started(ID)).expect_err("collision");
    match error {
        InvocationError::Write(message) => {
            assert!(
                message.starts_with(&format!(
                    "ULID collision on {}",
                    trail(temp.path()).display()
                )),
                "{message}"
            );
            assert!(
                message.ends_with("retry with a new invocation_id"),
                "{message}"
            );
        }
        other => panic!("expected a write error, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(trail(temp.path())).unwrap(),
        format!("{}\n", started(ID).to_jsonl_line()),
        "the failed collision must not touch the existing trail"
    );
}

#[test]
fn a_preexisting_trail_is_a_collision() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("kitty-ops")).unwrap();
    fs::write(trail(temp.path()), "existing content\n").unwrap();

    let error = InvocationWriter::new(temp.path())
        .write_started(&started(ID))
        .expect_err("collision");
    assert!(matches!(error, InvocationError::Write(_)), "{error:?}");
    assert_eq!(
        fs::read_to_string(trail(temp.path())).unwrap(),
        "existing content\n"
    );
}

#[test]
fn completed_appends_one_line_after_started() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    let record = started(ID);
    writer.write_started(&record).unwrap();

    let completed_record = completed(ID);
    let path = writer.write_completed(&completed_record).unwrap();
    assert_eq!(path, trail(temp.path()));
    let lines: Vec<String> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], record.to_jsonl_line());
    assert_eq!(lines[1], completed_record.to_jsonl_line());
    assert_eq!(lines[1].matches("\"event\"").count(), 1);
}

#[test]
fn a_second_completion_is_already_closed() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    writer.write_started(&started(ID)).unwrap();
    writer.write_completed(&completed(ID)).unwrap();

    match writer.write_completed(&completed_with(ID, |fields| {
        fields.outcome = OpOutcome::Failed;
    })) {
        Err(InvocationError::AlreadyClosed { invocation_id }) => {
            assert_eq!(invocation_id, ID);
            assert_eq!(
                InvocationError::AlreadyClosed {
                    invocation_id: ID.to_owned()
                }
                .to_string(),
                format!("Invocation {ID} is already closed.")
            );
        }
        other => panic!("expected AlreadyClosed, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(trail(temp.path()))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[test]
fn completing_a_missing_record_says_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let error = InvocationWriter::new(temp.path())
        .write_completed(&completed(ID))
        .expect_err("missing record");
    assert_eq!(
        error.to_string(),
        format!("Invocation record not found: {ID}")
    );
    assert!(!trail(temp.path()).exists());
}

#[test]
fn a_torn_tail_is_refused_and_left_byte_identical() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    let path = writer.write_started(&started(ID)).unwrap().path;
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(b"{\"torn\":");
    fs::write(&path, &bytes).unwrap();

    let error = writer
        .write_completed(&completed(ID))
        .expect_err("torn tail");
    let message = error.to_string();
    assert!(
        message.starts_with(&format!("Invocation record is unreadable: {ID} (")),
        "{message}"
    );
    assert!(
        message.contains("Error code: EVENT_LOG_TORN_TAIL."),
        "{message}"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes, "nothing may be appended");
}

#[test]
fn an_embedded_started_id_mismatch_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("kitty-ops")).unwrap();
    fs::write(
        trail(temp.path()),
        format!("{}\n", started(OTHER_ID).to_jsonl_line()),
    )
    .unwrap();

    let error = InvocationWriter::new(temp.path())
        .write_completed(&completed(ID))
        .expect_err("identity mismatch");
    assert_eq!(
        error.to_string(),
        format!("Invocation record identity mismatch: requested='{ID}', embedded='{OTHER_ID}'")
    );
    assert_eq!(
        fs::read_to_string(trail(temp.path()))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn a_trail_without_a_started_event_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("kitty-ops")).unwrap();
    let body = format!("{}\n", completed(ID).to_jsonl_line());
    fs::write(trail(temp.path()), &body).unwrap();

    let error = InvocationWriter::new(temp.path())
        .append_correlation_link(
            &InvocationId::parse(ID).unwrap(),
            CorrelationLink::artifact("docs/x.md"),
        )
        .expect_err("no started event");
    assert_eq!(
        error.to_string(),
        format!("Invocation record has no started event: {ID}")
    );
    assert_eq!(fs::read_to_string(trail(temp.path())).unwrap(), body);
}

#[test]
fn a_non_json_trail_is_unreadable() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("kitty-ops")).unwrap();
    fs::write(trail(temp.path()), "not json\n").unwrap();

    let error = InvocationWriter::new(temp.path())
        .write_completed(&completed(ID))
        .expect_err("unreadable");
    assert_eq!(
        error.to_string(),
        format!("Invocation record is unreadable: {ID}")
    );
    assert_eq!(
        fs::read_to_string(trail(temp.path())).unwrap(),
        "not json\n"
    );
}

#[cfg(unix)]
#[test]
fn a_same_root_symlink_substitution_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    let target = writer
        .write_started(&started(OTHER_ID))
        .expect("start other")
        .path;
    let link = trail(temp.path());
    std::os::unix::fs::symlink(target.file_name().expect("file name"), &link)
        .expect("plant symlink");

    let error = writer
        .write_completed(&completed(ID))
        .expect_err("symlink refused");
    assert_eq!(
        error.to_string(),
        format!(
            "Invocation record must not be a symbolic link: {}",
            link.display()
        )
    );
    assert_eq!(fs::read_to_string(&target).unwrap().lines().count(), 1);
}

#[cfg(unix)]
#[test]
fn a_symlink_escape_is_outside_the_trusted_root() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("kitty-ops")).unwrap();
    let outside = temp.path().join("outside.jsonl");
    fs::write(&outside, "attacker controlled\n").unwrap();
    std::os::unix::fs::symlink(&outside, trail(temp.path())).expect("plant symlink");

    let error = InvocationWriter::new(temp.path())
        .invocation_path(&InvocationId::parse(ID).unwrap())
        .expect_err("escape refused");
    match error {
        InvocationError::PathRefused(message) => {
            assert!(
                message.starts_with("Refusing to access path outside trusted roots: "),
                "{message}"
            );
            assert!(message.ends_with("outside.jsonl"), "{message}");
        }
        other => panic!("expected a path refusal, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "attacker controlled\n"
    );
}

#[test]
fn a_refused_index_append_is_a_warning_only() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("kitty-ops")).unwrap();
    let index = temp.path().join("kitty-ops/ops-index.jsonl");
    fs::write(&index, b"{\"torn\":").unwrap();

    let outcome = InvocationWriter::new(temp.path())
        .write_started(&started(ID))
        .expect("the Op still starts");
    let warning = outcome.index_warning.expect("the warning is reported");
    assert_eq!(warning.index_path, index);
    assert!(
        warning.message.contains("not written to"),
        "{}",
        warning.message
    );
    assert!(
        warning.message.contains("EVENT_LOG_TORN_TAIL"),
        "{}",
        warning.message
    );
    assert_eq!(fs::read(&index).unwrap(), b"{\"torn\":");
    assert!(outcome.path.is_file());
}

#[test]
fn the_closure_spine_is_append_only_and_tolerant() {
    let temp = tempfile::tempdir().unwrap();
    let first = completed_with(ID, |fields| {
        fields.outcome = OpOutcome::Abandoned;
        fields.closed_by = ClosedBy::DoctorSweep;
    });
    let second = completed_with(OTHER_ID, |fields| {
        fields.outcome = OpOutcome::Abandoned;
        fields.closed_by = ClosedBy::DoctorSweep;
    });

    let first_path = append_op_closure(temp.path(), &first).unwrap();
    let second_path = append_op_closure(temp.path(), &second).unwrap();
    assert_eq!(first_path, second_path);
    assert_eq!(first_path, op_closures_path(temp.path()));
    assert_eq!(
        fs::read_to_string(&first_path).unwrap(),
        format!("{}\n{}\n", first.to_jsonl_line(), second.to_jsonl_line())
    );

    // Absent spine → no closures. A corrupt line, a started line and a blank
    // line are skipped rather than fatal.
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(read_op_closures(empty.path()).unwrap(), Vec::new());
    let spine = op_closures_path(empty.path());
    fs::create_dir_all(spine.parent().unwrap()).unwrap();
    fs::write(
        &spine,
        format!(
            "not json\n{}\n{}\n\n",
            first.to_jsonl_line(),
            started(ID).to_jsonl_line()
        ),
    )
    .unwrap();
    let closures = read_op_closures(empty.path()).unwrap();
    assert_eq!(closures.len(), 1);
    assert_eq!(closures[0].invocation_id().as_str(), ID);
    assert_eq!(
        closed_invocation_ids(empty.path()).unwrap(),
        [InvocationId::parse(ID).unwrap()].into_iter().collect()
    );
}

#[test]
fn closed_ids_only_read_the_spine() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    writer.write_started(&started(ID)).unwrap();
    writer.write_completed(&completed(ID)).unwrap();
    assert!(writer.closed_ids().unwrap().is_empty());

    let sweep = completed_with(ID, |fields| {
        fields.closed_by = ClosedBy::DoctorSweep;
        fields.outcome = OpOutcome::Abandoned;
    });
    append_op_closure(temp.path(), &sweep).unwrap();
    assert_eq!(writer.closed_ids().unwrap().len(), 1);
}

#[test]
fn correlation_links_have_the_v2_shapes() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    let path = writer.write_started(&started(ID)).unwrap().path;

    writer
        .append_correlation_link(
            &InvocationId::parse(ID).unwrap(),
            CorrelationLink::artifact("spec.md").at("2026-06-10T21:00:00+00:00"),
        )
        .unwrap();
    writer
        .append_correlation_link(
            &InvocationId::parse(ID).unwrap(),
            CorrelationLink::commit("abc123").at("2026-06-10T21:00:01+00:00"),
        )
        .unwrap();

    let lines: Vec<String> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        lines[1],
        format!(
            "{{\"event\": \"artifact_link\", \"invocation_id\": \"{ID}\", \"at\": \"2026-06-10T21:00:00+00:00\", \"kind\": \"artifact\", \"ref\": \"spec.md\"}}"
        )
    );
    assert_eq!(
        lines[2],
        format!(
            "{{\"event\": \"commit_link\", \"invocation_id\": \"{ID}\", \"at\": \"2026-06-10T21:00:01+00:00\", \"sha\": \"abc123\"}}"
        )
    );
}

#[test]
fn a_correlation_link_needs_exactly_one_of_ref_or_sha() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    writer.write_started(&started(ID)).unwrap();
    let id = InvocationId::parse(ID).unwrap();

    let both = writer
        .append_correlation_link(
            &id,
            CorrelationLink {
                kind: "artifact".to_owned(),
                reference: Some("spec.md".to_owned()),
                sha: Some("abc123".to_owned()),
                at: None,
            },
        )
        .expect_err("both");
    assert_eq!(
        both.to_string(),
        "Exactly one of ref or sha must be provided"
    );

    let neither = writer
        .append_correlation_link(
            &id,
            CorrelationLink {
                kind: "artifact".to_owned(),
                reference: None,
                sha: None,
                at: None,
            },
        )
        .expect_err("neither");
    assert_eq!(
        neither.to_string(),
        "Exactly one of ref or sha must be provided"
    );
}

#[test]
fn a_correlation_link_on_a_missing_record_is_not_found() {
    let temp = tempfile::tempdir().unwrap();
    let error = InvocationWriter::new(temp.path())
        .append_correlation_link(
            &InvocationId::parse(ID).unwrap(),
            CorrelationLink::artifact("x"),
        )
        .expect_err("missing");
    assert_eq!(
        error.to_string(),
        format!("Invocation record not found: {ID}")
    );
}

#[test]
fn a_correlation_link_refuses_a_torn_tail() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    let path = writer.write_started(&started(ID)).unwrap().path;
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(b"{\"torn\":");
    fs::write(&path, &bytes).unwrap();

    let error = writer
        .append_correlation_link(
            &InvocationId::parse(ID).unwrap(),
            CorrelationLink::artifact("docs/x.md"),
        )
        .expect_err("torn");
    assert!(error.to_string().contains("EVENT_LOG_TORN_TAIL"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn completing_after_a_link_keeps_the_order_and_still_refuses() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    let path = writer.write_started(&started(ID)).unwrap().path;
    writer.write_completed(&completed(ID)).unwrap();
    writer
        .append_correlation_link(
            &InvocationId::parse(ID).unwrap(),
            CorrelationLink::artifact("spec.md").at("2026-06-10T21:00:00+00:00"),
        )
        .unwrap();

    assert!(matches!(
        writer.write_completed(&completed(ID)),
        Err(InvocationError::AlreadyClosed { .. })
    ));
    let events: Vec<String> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            value["event"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(events, ["started", "completed", "artifact_link"]);
}

#[test]
fn read_started_returns_the_durable_started_event() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    let record = started_with(ID, |fields| {
        fields.mission_id = Some("01KTK5JBD69FQ8XVRFV1J630MJ".to_owned());
        fields.wp_id = Some("WP01".to_owned());
        fields.model_id = Some("claude-opus-4-6".to_owned());
        fields.router_confidence = Some("exact".to_owned());
    });
    writer.write_started(&record).unwrap();
    assert_eq!(
        writer
            .read_started(&InvocationId::parse(ID).unwrap())
            .unwrap(),
        record
    );

    fs::write(
        trail(temp.path()),
        format!("{}\n", started(OTHER_ID).to_jsonl_line()),
    )
    .unwrap();
    let error = writer
        .read_started(&InvocationId::parse(ID).unwrap())
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Op record invocation_id '{OTHER_ID}' does not match requested '{ID}'")
    );

    let missing = writer
        .read_started(&InvocationId::parse(OTHER_ID).unwrap())
        .unwrap_err();
    assert_eq!(
        missing.to_string(),
        format!("Op record not found for invocation_id='{OTHER_ID}'")
    );
}

#[test]
fn read_started_mode_keeps_absence_known_and_undetermined_apart() {
    let temp = tempfile::tempdir().unwrap();
    let writer = InvocationWriter::new(temp.path());
    let id = InvocationId::parse(ID).unwrap();

    writer.write_started(&started(ID)).unwrap();
    assert_eq!(
        writer.read_started_mode(&id).unwrap(),
        RecordedMode::Known(ModeOfWork::TaskExecution)
    );

    let mut line: serde_json::Value = serde_json::from_str(&started(ID).to_jsonl_line()).unwrap();
    line.as_object_mut().unwrap().remove("mode_of_work");
    fs::write(trail(temp.path()), format!("{line}\n")).unwrap();
    assert_eq!(writer.read_started_mode(&id).unwrap(), RecordedMode::Absent);

    line.as_object_mut().unwrap().insert(
        "mode_of_work".to_owned(),
        serde_json::Value::String("bogus".to_owned()),
    );
    fs::write(trail(temp.path()), format!("{line}\n")).unwrap();
    assert_eq!(
        writer.read_started_mode(&id).unwrap(),
        RecordedMode::Undetermined(serde_json::Value::String("bogus".to_owned()))
    );

    let missing = writer
        .read_started_mode(&InvocationId::parse(OTHER_ID).unwrap())
        .unwrap_err();
    assert_eq!(
        missing.to_string(),
        format!("Invocation record not found: {OTHER_ID}")
    );
}

#[test]
fn normalise_ref_resolves_inside_outside_and_relative_refs() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("docs")).unwrap();
    let artifact = repo.join("docs/output.md");
    fs::write(&artifact, "x").unwrap();

    assert_eq!(
        normalise_ref(&artifact.to_string_lossy(), &repo),
        "docs/output.md"
    );
    // Repo-relative refs resolve from the repository root, not the caller's cwd.
    assert_eq!(normalise_ref("docs/output.md", &repo), "docs/output.md");
    assert_eq!(normalise_ref(".", &repo), ".");

    let outside = temp.path().join("out.log");
    fs::write(&outside, "x").unwrap();
    let normalised = normalise_ref(&outside.to_string_lossy(), &repo);
    assert_eq!(
        normalised,
        outside.canonicalize().unwrap().to_string_lossy()
    );

    for verbatim in [
        "https://example.com/x",
        "file:///tmp/x",
        "s3://bucket/key",
        "not/a/real/path\u{0}",
    ] {
        assert_eq!(normalise_ref(verbatim, &repo), verbatim);
    }

    #[cfg(unix)]
    {
        let first = temp.path().join("loop-a");
        let second = temp.path().join("loop-b");
        std::os::unix::fs::symlink(&second, &first).unwrap();
        std::os::unix::fs::symlink(&first, &second).unwrap();
        let loop_ref = first.to_string_lossy().into_owned();
        assert_eq!(normalise_ref(&loop_ref, &repo), loop_ref);
    }
}
