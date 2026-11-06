//! Clarification collection only: real store, conditional/fresh read phases,
//! exact IDs and text boundaries. NOT full Source-default acceptance approval.
#![cfg(unix)]
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use sk_status::acceptance::source::{SourceAcceptanceOptions, evaluate_source_acceptance};
use sk_workspace::acceptance_source::{MatrixInput, collect_source_acceptance_facts};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};
fn roots() -> [(&'static str, &'static str); 9] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}
fn child(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_ACCEPT_DECISION_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[derive(Debug, Eq, PartialEq)]
struct Footprint {
    path: PathBuf,
    mode: u32,
    inode: u64,
    mtime: std::time::SystemTime,
    bytes: Option<Vec<u8>>,
}
fn footprint(path: &Path) -> Vec<Footprint> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path).unwrap();
    let mut out = vec![Footprint {
        path: path.to_path_buf(),
        mode: meta.mode(),
        inode: meta.ino(),
        mtime: meta.modified().unwrap(),
        bytes: if meta.is_file() {
            Some(fs::read(path).unwrap())
        } else {
            None
        },
    }];
    if meta.is_dir()
        && let Ok(children) = fs::read_dir(path)
    {
        for entry in children {
            out.extend(footprint(&entry.unwrap().path()));
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}
fn mission(root: &Path) -> sk_mission::Mission {
    sk_mission::list_missions(root)
        .unwrap()
        .into_iter()
        .find(|m| m.slug().as_str() == "001-decision-probe")
        .unwrap()
}
fn error_class(error: &sk_workspace::WorkspaceError) -> &'static str {
    match error {
        sk_workspace::WorkspaceError::DecisionIndex(error) => match error.as_ref() {
            sk_workspace::decision_store::DecisionIndexLoadError::Read(_) => {
                "DecisionIndexReadError"
            }
            sk_workspace::decision_store::DecisionIndexLoadError::Probe { source, .. }
                if source.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                "PermissionError"
            }
            _ => "OSError",
        },
        sk_workspace::WorkspaceError::ArtifactEncoding { .. } => "ArtifactEncodingError",
        _ => "unexpected",
    }
}
#[test]
fn clarification_collection_matches_source_read_phases_and_exact_ids() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_ACCEPT_DECISION_NATIVE") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (k, n) in roots() {
            assert_eq!(std::env::var_os(k).unwrap(), root.join(n));
        }
        assert_eq!(std::env::var_os("PYTHONPATH").unwrap(), repo.join("src"));
        assert_eq!(
            std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
            repo.join("packs")
        );
        let expected: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let rows = expected["rows"].as_array().unwrap();
        assert!(rows.len() > 50);
        for row in rows {
            let project = Path::new(row["project"].as_str().unwrap());
            let before = footprint(project);
            let m = mission(project);
            let result = collect_source_acceptance_facts(project, &m, MatrixInput::NotRequired);
            if let Some(kind) = row["error"].as_str() {
                let error = result.expect_err("Source failed at this phase");
                assert_eq!(error_class(&error), kind, "{}: {error}", row["name"]);
                if row["compare_text"].as_bool() == Some(true) {
                    assert_eq!(
                        error.to_string(),
                        row["message"].as_str().unwrap(),
                        "{}",
                        row["name"]
                    );
                }
            } else {
                let facts = result.unwrap();
                assert!(facts.decision_statuses_available);
                let observed: Vec<Vec<u32>> = facts
                    .decision_statuses
                    .keys()
                    .map(|id| id.codepoints().to_vec())
                    .collect();
                assert_eq!(
                    serde_json::to_value(observed).unwrap(),
                    row["keys"],
                    "{} exact lookup IDs",
                    row["name"]
                );
                let spec = facts
                    .clarifications
                    .iter()
                    .find(|scan| scan.path.ends_with("spec.md"))
                    .unwrap();
                let ids: Vec<Vec<u32>> =
                    sk_status::acceptance::source::clarification_decision_ids(&spec.text)
                        .iter()
                        .map(|id| id.chars().map(u32::from).collect())
                        .collect();
                assert_eq!(serde_json::to_value(ids).unwrap(), row["ids"]);
                let summary =
                    evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
                assert_eq!(
                    serde_json::to_value(&summary.needs_clarification).unwrap(),
                    row["blocked"],
                    "{}",
                    row["name"]
                );
                fs::write(
                    root.join(format!(
                        "{}.native-summary.json",
                        row["name"].as_str().unwrap()
                    )),
                    serde_json::to_vec(&summary.to_json()).unwrap(),
                )
                .unwrap();
            }
            assert_eq!(
                footprint(project),
                before,
                "{} read-only footprint",
                row["name"]
            );
        }
        let expected_ignored: Vec<i32> =
            serde_json::from_value(expected["ignored_errnos"].clone()).unwrap();
        let native_ignored: Vec<i32> = (0..256)
            .filter(|n| {
                sk_kernel::paths::source_exists_ignores(&std::io::Error::from_raw_os_error(*n))
            })
            .collect();
        assert_eq!(native_ignored, expected_ignored);
        assert!(!sk_kernel::paths::source_path_exists(Path::new("bad\0path")).unwrap());
        let stream = &expected["stream"];
        let project = Path::new(stream["project"].as_str().unwrap());
        let feature = project.join("kitty-specs/001-decision-probe");
        let index = feature.join("decisions/index.json");
        let plan = feature.join("plan.md");
        let open: Vec<u8> = serde_json::from_value(stream["payload_open"].clone()).unwrap();
        let resolved: Vec<u8> = serde_json::from_value(stream["payload_resolved"].clone()).unwrap();
        let text: Vec<u8> = serde_json::from_value(stream["plan"].clone()).unwrap();
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let identity = |p: &Path| {
            let m = fs::metadata(p).unwrap();
            assert!(m.file_type().is_fifo());
            (m.ino(), m.mode())
        };
        let before = [identity(&index), identity(&plan)];
        let index_writer = index.clone();
        let plan_writer = plan.clone();
        let writer = std::thread::spawn(move || {
            for (path, data) in [
                (index_writer.clone(), open),
                (plan_writer, text),
                (index_writer, resolved),
            ] {
                let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
                file.write_all(&data).unwrap();
            }
        });
        let m = mission(project);
        let facts = collect_source_acceptance_facts(project, &m, MatrixInput::NotRequired).unwrap();
        writer.join().unwrap();
        let summary = evaluate_source_acceptance(&facts, SourceAcceptanceOptions::default());
        assert_eq!(
            serde_json::to_value(&summary.needs_clarification).unwrap(),
            stream["blocked"],
            "fresh per-file reads, not a final global snapshot"
        );
        assert_eq!([identity(&index), identity(&plan)], before);
        println!(
            "{} ordinary read/marker cases, 256 errno policies and Source FIFO fresh-read handshake",
            rows.len()
        );
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("acceptance decision evidence: {}", root.display());
    let source = child(&root, &repo, oracle::python())
        .args(["-c", include_str!("oracle/acceptance_decisions.py")])
        .output()
        .unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    fs::write(
        root.join("source.status"),
        format!("{:?}\n", source.status.code()),
    )
    .unwrap();
    assert!(
        source.status.success(),
        "Source FIRST: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    let native = child(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "clarification_collection_matches_source_read_phases_and_exact_ids",
            "--nocapture",
        ])
        .env("SK_ACCEPT_DECISION_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    fs::write(
        root.join("native.status"),
        format!("{:?}\n", native.status.code()),
    )
    .unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}
