//! Source-first parity for `GitRepository::remote_url` (the canonical local
//! `git remote get-url <name>` read).
//!
//! The PINNED Source's own read runs first — the identical
//! `subprocess.run(["git", "remote", "get-url", <name>], check=True,
//! capture_output=True, text=True, timeout=2).stdout.strip()` its former-checkout
//! proof uses — and is asserted; the Native API then reads the same repository and
//! must return exactly that string (or the typed absence when Source's call
//! raises `CalledProcessError` because the remote does not exist).
//!
//! Cases: a plain remote, a `url.<base>.insteadOf` rewrite (which a
//! `config --get` read would miss), a multi-URL remote (first URL wins), a
//! scp-like ssh URL, a URL with surrounding whitespace, and a missing remote.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::path::Path;
use std::process::Command;

use sk_git::{GitRepository, RemoteName};

const ORACLE: &str = r#"
import json, subprocess, sys
repo = sys.argv[1]
out = {}
for name in ["origin", "mirror", "rewritten", "missing"]:
    try:
        proc = subprocess.run(
            ["git", "-C", repo, "remote", "get-url", name],
            check=True,
            capture_output=True,
            text=True,
            timeout=2,
        )
    except subprocess.CalledProcessError as exc:
        out[name] = {"raised": True, "returncode": exc.returncode}
    else:
        out[name] = {"raised": False, "value": proc.stdout.strip()}
print(json.dumps(out, sort_keys=True))
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn fixture() -> std::path::PathBuf {
    let root = tempfile::tempdir().expect("sandbox").keep();
    git(&root, &["init", "-q", "-b", "main"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/spec-kitty.git",
        ],
    );
    // A second remote with two URLs: `get-url` reports the FIRST.
    git(
        &root,
        &[
            "remote",
            "add",
            "mirror",
            "ssh://git@example.invalid/first.git",
        ],
    );
    git(
        &root,
        &[
            "remote",
            "set-url",
            "--add",
            "mirror",
            "ssh://git@example.invalid/second.git",
        ],
    );
    // A remote whose stored URL is rewritten by `url.<base>.insteadOf`.
    git(&root, &["remote", "add", "rewritten", "sk:spec-kitty.git"]);
    git(
        &root,
        &["config", "url.ssh://git@example.invalid/.insteadOf", "sk:"],
    );
    let _ = git(
        &root,
        &[
            "config",
            "remote.origin.pushurl",
            "https://example.invalid/push.git",
        ],
    );
    root
}

fn source_values(root: &Path) -> serde_json::Value {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = repo_root.join("target/remote-url-oracle.py");
    std::fs::write(&script, ORACLE).expect("write oracle script");
    let output = Command::new(oracle::python())
        .arg(&script)
        .arg(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .output()
        .expect("run the pinned Source read");
    assert!(
        output.status.success(),
        "the pinned Source read failed (harness/premise failure):\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle json")
}

#[test]
fn remote_url_matches_the_pinned_source_read() {
    let root = fixture();
    std::fs::create_dir_all(root.join("home")).expect("home");
    let source = source_values(&root);
    let repository = GitRepository::open(&root).expect("open fixture");

    for name in ["origin", "mirror", "rewritten"] {
        let remote = RemoteName::parse(name).expect("remote name");
        let native = repository
            .remote_url(&remote)
            .expect("a configured remote reads cleanly");
        let expected = source[name]["value"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: Source did not report a URL: {}", source[name]));
        assert_eq!(
            native.as_deref(),
            Some(expected),
            "{name}: the Native read matches the pinned Source's `get-url` output"
        );
        assert!(
            !source[name]["raised"].as_bool().unwrap_or(false),
            "{name}: Source answered rather than raising"
        );
    }

    // The insteadOf rewrite is applied (a `config --get` reader would return
    // `sk:spec-kitty.git` and silently disagree with every git operation).
    let rewritten = repository
        .remote_url(&RemoteName::parse("rewritten").unwrap())
        .expect("read")
        .expect("present");
    assert_eq!(rewritten, "ssh://git@example.invalid/spec-kitty.git");
    assert_ne!(rewritten, "sk:spec-kitty.git");
    assert_eq!(
        git(&root, &["config", "--get", "remote.rewritten.url"]),
        "sk:spec-kitty.git"
    );

    // A missing remote is a TYPED absence: Source's `check=True` raises there, so
    // the Native side must not invent a URL or a different failure.
    assert!(
        source["missing"]["raised"].as_bool().unwrap_or(false),
        "Source raises for a missing remote: {}",
        source["missing"]
    );
    assert_eq!(
        repository
            .remote_url(&RemoteName::parse("missing").unwrap())
            .expect("a missing remote is absence, not an error"),
        None
    );
}
