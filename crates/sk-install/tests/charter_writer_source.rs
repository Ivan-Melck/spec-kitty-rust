//! Source-first charter exception boundaries, Unicode replacement and read-only preview.
#![cfg(unix)]
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};
const BODY: &str = "abcdefghij🙂 constitution\nCONSTITUTION conſtitution constİtutıon\n";
fn roots() -> [(&'static str, &'static str); 11] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("XDG_CONFIG_DIRS", "config-dirs"),
        ("XDG_DATA_DIRS", "data-dirs"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}
fn child(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "umask 027; exec \"$@\"", "isolated"])
        .arg(program)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_CHARTER_WRITER_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8");
    for (key, name) in roots() {
        command.env(key, root.join(name));
    }
    command
}
#[derive(Debug, PartialEq)]
struct FileState {
    bytes: Vec<u8>,
    mode: u32,
    symlink: bool,
    inode: u64,
    modified: (i64, i64),
}
fn snapshot(project: &Path) -> BTreeMap<String, FileState> {
    fn walk(root: &Path, directory: &Path, out: &mut BTreeMap<String, FileState>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                walk(root, &path, out);
                continue;
            }
            let link = metadata.file_type().is_symlink();
            use std::os::unix::ffi::OsStrExt;
            let bytes = if link {
                fs::read_link(&path)
                    .unwrap()
                    .as_os_str()
                    .as_bytes()
                    .to_vec()
            } else {
                fs::read(&path).unwrap()
            };
            out.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                FileState {
                    bytes,
                    mode: metadata.mode() & 0o7777,
                    symlink: link,
                    inode: metadata.ino(),
                    modified: (metadata.mtime(), metadata.mtime_nsec()),
                },
            );
        }
    }
    let mut out = BTreeMap::new();
    walk(project, project, &mut out);
    out
}
fn fixture(project: &Path, case: &str) -> Option<PathBuf> {
    let directory = project.join(".kittify/charter");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("z.md"), b"constitution later\n").unwrap();
    match case {
        "symlink" => {
            fs::write(project.join("external"), b"constitution target\n").unwrap();
            symlink("../../external", directory.join("a.md")).unwrap();
        }
        "symlink-dir" => {
            fs::create_dir_all(project.join("external")).unwrap();
            fs::write(project.join("external/a.md"), b"constitution target\n").unwrap();
            symlink("../../external", directory.join("a")).unwrap();
        }
        "oserror" => {
            fs::create_dir_all(directory.join("locked")).unwrap();
            fs::write(directory.join("locked/a.md"), b"constitution locked\n").unwrap();
        }
        _ => fs::write(
            directory.join("a.md"),
            if case == "unicode" {
                BODY
            } else {
                "constitution first\n"
            },
        )
        .unwrap(),
    }
    let artifact =
        (case == "collision").then(|| directory.join(format!(".a.md.tmp{}", std::process::id())));
    if let Some(path) = &artifact {
        fs::write(path, b"operator artifact\x00\xff").unwrap();
    }
    fn modes(path: &Path) {
        let metadata = fs::symlink_metadata(path).unwrap();
        if metadata.file_type().is_symlink() {
            return;
        }
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if metadata.is_dir() { 0o775 } else { 0o664 }),
        )
        .unwrap();
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                modes(&entry.unwrap().path());
            }
        }
    }
    modes(project);
    if case == "oserror" {
        fs::set_permissions(directory.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();
    }
    artifact
}
fn native(root: &Path, repo: &Path) {
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in roots() {
        assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
    }
    assert_eq!(std::env::var_os("PYTHONPATH").unwrap(), repo.join("src"));
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
        repo.join("packs")
    );
    let rows: Vec<Value> =
        serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
    let callback = sk_install::migrations::completion_modern::rescue_spine::IMPLEMENTATIONS
        .iter()
        .find(|migration| migration.id == "3.1.1_charter_rename")
        .unwrap();
    for (number, row) in rows.iter().enumerate() {
        let case = row["case"].as_str().unwrap();
        let preview = row["preview"].as_bool().unwrap();
        let project = root.join(format!("native-{number}"));
        let artifact = fixture(&project, case);
        let mut before = snapshot(&project);
        let result = if preview {
            sk_install::migrations::completion_modern::rescue_spine::preview_charter_rename(
                &project,
            )
        } else {
            (callback.apply)(&project)
        };
        if row.get("exception").is_some() {
            let error = result.expect_err(
                "Source ValueError must escape, not become a returned failure/continuation",
            );
            assert_eq!(error.code(), "source_write");
            if case == "collision" {
                // Each artifact names its ACTUAL executing PID. Compare Source's
                // static message with the corresponding real Native path; never
                // substitute/fabricate one process's PID for another's.
                assert_eq!(
                    error.to_string(),
                    format!(
                        "Refusing to replace an unproven atomic writer artifact: {}",
                        artifact.as_ref().unwrap().display()
                    )
                );
            } else {
                assert_eq!(
                    error
                        .to_string()
                        .replace(project.to_str().unwrap(), "<project>"),
                    row["relative_message"].as_str().unwrap()
                );
            }
            assert_eq!(
                snapshot(&project),
                before,
                "uncaught failure touched later/foreign files"
            );
        } else {
            let outcome = result.unwrap();
            assert_eq!(outcome.success, row["success"].as_bool().unwrap(), "{case}");
            assert_eq!(json!(outcome.changes), row["changes"], "{case}");
            assert_eq!(json!(outcome.warnings), row["warnings"], "{case}");
            assert_eq!(
                outcome.errors.len(),
                row["errors"].as_array().unwrap().len(),
                "{case}"
            );
            if case != "oserror" {
                assert_eq!(json!(outcome.errors), row["errors"]);
            }
            // Generic IO strerror remains OPEN; this asserts the actual catch-and-
            // continue class and footprint, NOT byte parity for that error prose.
        }
        let mut after = snapshot(&project);
        let raw = |states: &BTreeMap<String, FileState>| -> BTreeMap<String, Value> {
            states
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        json!({
                            "bytes": value.bytes, "mode": value.mode, "symlink": value.symlink,
                            "inode": value.inode, "mtime_seconds": value.modified.0,
                            "mtime_subsec_ns": value.modified.1
                        }),
                    )
                })
                .collect()
        };
        fs::write(
            root.join(format!("native-{number}-footprint.json")),
            serde_json::to_vec_pretty(&json!({"before": raw(&before), "after": raw(&after)}))
                .unwrap(),
        )
        .unwrap();
        if let Some(path) = artifact {
            assert!(row["artifact_pid"].as_u64().unwrap() > 0);
            assert_eq!(
                path,
                sk_kernel::generated_temporary_path(&project.join(".kittify/charter/a.md"))
            );
            let key = path.strip_prefix(&project).unwrap().to_str().unwrap();
            assert_eq!(before.get(key), after.get(key), "PID artifact changed");
            before.remove(key);
            after.remove(key);
        }
        let rewritten = row["rewritten"].as_array().unwrap();
        let files: BTreeMap<String, Value> = after.into_iter().map(|(key, value)| {
            let old = before.get(&key).expect("no invented extra artifacts");
            let written = rewritten.iter().any(|path| path.as_str() == Some(key.as_str()));
            if written {
                assert_ne!(old.inode, value.inode, "generated output must actually publish");
            } else {
                assert_eq!(old, &value, "untouched/foreign/preview metadata must stay exact");
            }
            let mtime_invariant = if written { None } else { Some(old.modified == value.modified) };
            let encoded = json!({"bytes": value.bytes, "mode": value.mode, "symlink": value.symlink,
                "same_inode": old.inode == value.inode, "same_mtime": mtime_invariant});
            (key, encoded)
        }).collect();
        assert_eq!(json!(files), row["files"], "{case} preview={preview}");
    }
    println!(
        "7 actual Source callback cases: exception/continue/Unicode/directory-link/preview footprints"
    );
}
#[test]
fn charter_write_failure_phase_and_unicode_match_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_CHARTER_WRITER_NATIVE") {
        native(Path::new(&root), &repo);
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    eprintln!("charter writer evidence: {}", root.display());
    let source = child(&root, &repo, oracle::python())
        .args(["-c", include_str!("oracle/charter_writer.py")])
        .output()
        .unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    fs::write(
        root.join("source.status"),
        format!("{:?}", source.status.code()),
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
            "charter_write_failure_phase_and_unicode_match_source",
            "--nocapture",
        ])
        .env("SK_CHARTER_WRITER_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    fs::write(
        root.join("native.status"),
        format!("{:?}", native.status.code()),
    )
    .unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}
