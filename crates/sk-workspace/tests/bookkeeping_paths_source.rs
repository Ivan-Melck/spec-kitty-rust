//! Source-first, exact codepoint and native-byte classification; not a
//! certification of complete acceptance or destructive lifecycle gates.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use std::{fs, path::Path, process::Command};
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
        .env("SK_BOOKKEEPING_ROOT", root)
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
#[test]
fn canonical_bookkeeping_and_dossier_units_match_actual_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_BOOKKEEPING_NATIVE") {
        let root = std::path::PathBuf::from(root);
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
        assert!(rows.len() > 9000);
        for row in rows {
            let points: Vec<u32> = serde_json::from_value(row["points"].clone()).unwrap();
            let exact = sk_kernel::PythonString::from_codepoints(&points).unwrap();
            assert_eq!(
                sk_workspace::coherence::is_self_bookkeeping_churn(exact.codepoints()),
                row["bookkeeping"].as_bool().unwrap(),
                "exact points {points:?}"
            );
            assert_eq!(
                sk_status::preflight::is_dossier_snapshot(exact.codepoints()),
                row["dossier"].as_bool().unwrap(),
                "dossier points {points:?}"
            );
        }
        let raw = expected["raw_rows"].as_array().unwrap();
        assert_eq!(raw.len(), 768);
        for row in raw {
            let bytes: Vec<u8> = serde_json::from_value(row["bytes"].clone()).unwrap();
            assert_eq!(
                sk_workspace::coherence::is_self_bookkeeping_churn(&bytes),
                row["bookkeeping"].as_bool().unwrap(),
                "native bytes {bytes:?}"
            );
            assert_eq!(
                sk_status::preflight::is_dossier_snapshot(&bytes),
                row["dossier"].as_bool().unwrap(),
                "dossier native bytes {bytes:?}"
            );
        }
        println!(
            "{} exact path cases including all2048Cs; {} raw-byte cases",
            rows.len(),
            raw.len()
        );
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("bookkeeping evidence: {}", root.display());
    let source = child(&root, &repo, oracle::python())
        .args(["-c", include_str!("oracle/bookkeeping_paths.py")])
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
            "canonical_bookkeeping_and_dossier_units_match_actual_source",
            "--nocapture",
        ])
        .env("SK_BOOKKEEPING_NATIVE", &root)
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
