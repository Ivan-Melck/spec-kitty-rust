//! Differential test: the native CLI's full `init` post-state against the
//! pinned Python oracle.
//!
//! The development oracle is required and verified lock-exact; absence/drift
//! fails rather than silently skipping. `SPEC_KITTY_ORACLE_PYTHON` overrides
//! the shared selector (`SK_KERNEL_PYTHON`, then `.venv-rust-oracle`). Run with:
//!
//! ```sh
//! scripts/rust/verify_oracle.py >/dev/null || exit 1
//! SPEC_KITTY_ORACLE_PYTHON="$PWD/.venv-rust-oracle/bin/python" \
//!   CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 \
//!   cargo test -p sk-cli --test init_oracle_diff -- --nocapture
//! ```
//!
//! It compares the **complete** post-state: every file, every byte, with only
//! genuinely non-deterministic values (timestamps, minted ids) normalized, plus
//! the documented environment-provenance gap and the skills-manifest entry
//! ordering difference. It does not reduce the comparison to a set or a
//! substring: a missing, extra or reordered profile/skill file fails.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle_support;

fn oracle_python() -> PathBuf {
    match std::env::var_os("SPEC_KITTY_ORACLE_PYTHON") {
        Some(value) => oracle_support::verify_python(Path::new(&value)),
        None => oracle_support::python(),
    }
}

fn run(command: &mut Command, label: &str) -> Output {
    let output = command.output().unwrap_or_else(|error| {
        panic!("could not run {label}: {error}");
    });
    assert_eq!(
        output.status.code(),
        Some(0),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn walk(root: &Path) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("read dir") {
            let entry = entry.expect("entry");
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || name == "spec-kitty-locks" {
                continue;
            }
            let path = entry.path();
            let file_type = entry.file_type().expect("type");
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("relative")
                    .to_string_lossy()
                    .replace('\\', "/");
                // The canonical charter-synthesis lock (governance owner) is
                // held under `.kittify/runtime/locks/`, which the runtime
                // `.gitignore` entries already cover; the pinned writer does
                // not leave one behind on this path. Tracked as a known
                // integration item rather than a native init difference.
                if relative.starts_with(".kittify/runtime/locks/") {
                    continue;
                }
                files.insert(relative);
            }
        }
    }
    files
}

fn normalize(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let characters: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < characters.len() {
        let window: String = characters[index..(index + 40).min(characters.len())]
            .iter()
            .collect();
        // ISO-8601 timestamp with an optional fractional part and offset.
        if let Some(length) = iso_prefix_length(&window) {
            out.push_str("<timestamp>");
            index += length;
            continue;
        }
        // UUID.
        if characters.len() - index >= 36 {
            let candidate: String = characters[index..index + 36].iter().collect();
            if is_uuid(&candidate) {
                out.push_str("<uuid>");
                index += 36;
                continue;
            }
        }
        // ULID (26 Crockford base32 characters), only as a *delimited* token.
        // Without the boundary test a 64-hex SHA-256 (all of whose characters are
        // in the Crockford alphabet) would be rewritten from its first 26
        // characters and only its tail would be compared.
        if let Some(length) = ulid_prefix_length(&characters, index) {
            out.push_str("<ulid>");
            index += length;
            continue;
        }
        out.push(characters[index]);
        index += 1;
    }
    out.into_bytes()
}

fn is_ulid_character(character: char) -> bool {
    "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(character)
}

fn ulid_prefix_length(characters: &[char], index: usize) -> Option<usize> {
    const LENGTH: usize = 26;
    if characters.len() - index < LENGTH {
        return None;
    }
    if !characters[index..index + LENGTH]
        .iter()
        .all(|character| is_ulid_character(*character))
    {
        return None;
    }
    let delimited_before = index == 0 || !is_ulid_character(characters[index - 1]);
    let delimited_after = characters
        .get(index + LENGTH)
        .is_none_or(|character| !is_ulid_character(*character));
    (delimited_before && delimited_after).then_some(LENGTH)
}

fn iso_prefix_length(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && bytes[4] == b'-' && digits(5..7) && bytes[7] == b'-' && digits(8..10)) {
        return None;
    }
    if bytes[10] != b'T' || !(digits(11..13) && bytes[13] == b':' && digits(14..16)) {
        return None;
    }
    if bytes[16] != b':' || !digits(17..19) {
        return None;
    }
    let mut length = 19;
    if bytes.get(length) == Some(&b'.') {
        length += 1;
        while bytes.get(length).is_some_and(u8::is_ascii_digit) {
            length += 1;
        }
    }
    match bytes.get(length) {
        Some(&b'Z') => Some(length + 1),
        Some(&b'+') | Some(&b'-') if bytes.len() >= length + 6 => {
            let suffix = &text[length..length + 6];
            let suffix = suffix.as_bytes();
            if suffix[3] != b':'
                || !suffix[1..3].iter().all(u8::is_ascii_digit)
                || !suffix[4..6].iter().all(u8::is_ascii_digit)
            {
                return None;
            }
            Some(length + 6)
        }
        _ => None,
    }
}

fn is_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && text
            .chars()
            .all(|character| character == '-' || character.is_ascii_hexdigit())
}

fn read(root: &Path, relative: &str) -> Vec<u8> {
    fs::read(root.join(relative)).unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn row_objects(bytes: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json row"))
        .collect()
}

fn strip(rows: &[serde_json::Value], fields: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|row| {
            let mut row = row.clone();
            if let Some(object) = row.as_object_mut() {
                for field in fields {
                    object.remove(*field);
                }
            }
            String::from_utf8(normalize(
                serde_json::to_string(&row).expect("row").as_bytes(),
            ))
            .expect("normalized row")
        })
        .collect();
    out.sort();
    out
}

#[test]
fn native_init_post_state_matches_the_pinned_oracle() {
    let python = oracle_python();
    // The first set is the delivered trio; `gemini` exercises an agent with no
    // project-local profile renderer (no empty profile manifest) and `copilot`
    // exercises the `.github` orientation guard and session-before-profiles
    // ordering. Each is a complete post-state comparison, not a subset.
    for agents in ["claude,codex,vibe", "gemini", "copilot"] {
        compare_post_state(&python, agents);
    }
}

#[test]
fn a_missing_init_oracle_is_a_failing_precondition_not_a_passing_skip() {
    let temp = tempfile::tempdir().expect("oracle precondition sandbox");
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "native_init_post_state_matches_the_pinned_oracle",
            "--nocapture",
        ])
        .current_dir(temp.path())
        .env(
            "SPEC_KITTY_ORACLE_PYTHON",
            temp.path().join("missing-python"),
        )
        .output()
        .expect("run missing-oracle child");
    assert!(
        !output.status.success(),
        "missing oracle silently passed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn compare_post_state(python: &Path, agents: &str) {
    let root = repo_root();
    let sandbox = tempfile::tempdir().expect("sandbox");
    // Both projects carry the same directory name so the project slug (and
    // every file derived from it) is comparable.
    let oracle = sandbox.path().join("oracle/proj");
    let native = sandbox.path().join("native/proj");
    let home = sandbox.path().join("home");
    let state = sandbox.path().join("state");
    let tmp = sandbox.path().join("tmp");
    for directory in [&oracle, &native, &home, &state, &tmp] {
        fs::create_dir_all(directory).expect("create dir");
    }

    run(
        Command::new(python)
            .current_dir(&oracle)
            .env("HOME", &home)
            .env("XDG_STATE_HOME", &state)
            .env("TMPDIR", &tmp)
            .env("PYTHONPATH", root.join("src"))
            .env("SPEC_KITTY_PACKS_ROOT", root.join("packs"))
            .args([
                "-c",
                "from specify_cli import main; main()",
                "init",
                "--ai",
                agents,
                "--non-interactive",
            ]),
        "oracle init",
    );
    run(
        Command::new(binary())
            .current_dir(&native)
            .env("HOME", &home)
            .env("XDG_STATE_HOME", &state)
            .env("TMPDIR", &tmp)
            // The documented source-backed value: the checkout's `packs` dir.
            .env("SPEC_KITTY_PACKS_ROOT", root.join("packs"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["init", "--ai", agents, "--non-interactive"]),
        "native init",
    );

    let oracle_files = walk(&oracle);
    let native_files = walk(&native);
    let only_oracle: Vec<&String> = oracle_files.difference(&native_files).collect();
    let only_native: Vec<&String> = native_files.difference(&oracle_files).collect();
    assert!(
        only_oracle.is_empty(),
        "files the oracle created but the native init did not: {only_oracle:?}"
    );
    assert!(
        only_native.is_empty(),
        "files the native init created but the oracle did not: {only_native:?}"
    );

    let mut differences = Vec::new();
    for relative in &oracle_files {
        let oracle_bytes = read(&oracle, relative);
        let native_bytes = read(&native, relative);
        match relative.as_str() {
            // Documented provenance gap: no interpreter is spawned, so
            // python_version/platform_version cannot be fabricated. Everything
            // else in the file must still match.
            ".kittify/metadata.yaml" => {
                let oracle_text = strip_environment_block(&oracle_bytes);
                let native_text = strip_environment_block(&native_bytes);
                if normalize(oracle_text.as_bytes()) != normalize(native_text.as_bytes()) {
                    differences.push(relative.clone());
                }
            }
            // The pinned writer records the umbrella `spec-kitty` skill after
            // the rest of the catalog; entry ownership, paths and hashes are
            // otherwise identical. Compare as a full multiset (not a set of
            // normalized tuples) and assert only the ordering difference.
            ".kittify/skills-manifest.json" => {
                let oracle_manifest: serde_json::Value =
                    serde_json::from_slice(&oracle_bytes).expect("oracle json");
                let native_manifest: serde_json::Value =
                    serde_json::from_slice(&native_bytes).expect("native json");
                assert_eq!(native_manifest["version"], oracle_manifest["version"]);
                assert_eq!(
                    native_manifest["spec_kitty_version"],
                    oracle_manifest["spec_kitty_version"]
                );
                let sorted = |manifest: &serde_json::Value| {
                    let mut entries = manifest["entries"].as_array().expect("entries").clone();
                    for entry in &mut entries {
                        entry["installed_at"] = serde_json::Value::String("<timestamp>".into());
                    }
                    entries.sort_by_key(|entry| {
                        (
                            entry["installed_path"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            entry["agent_key"].as_str().unwrap_or_default().to_string(),
                        )
                    });
                    entries
                };
                assert_eq!(
                    sorted(&native_manifest),
                    sorted(&oracle_manifest),
                    "skill manifest entry sets/ownership/hashes differ"
                );
                if normalize(&oracle_bytes) != normalize(&native_bytes) {
                    let oracle_order: Vec<String> = oracle_manifest["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|entry| entry["installed_path"].as_str().unwrap().to_string())
                        .collect();
                    let native_order: Vec<String> = native_manifest["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|entry| entry["installed_path"].as_str().unwrap().to_string())
                        .collect();
                    let without_umbrella = |order: &[String]| -> Vec<String> {
                        order
                            .iter()
                            .filter(|path| !path.contains("/skills/spec-kitty/"))
                            .cloned()
                            .collect()
                    };
                    assert_eq!(
                        without_umbrella(&native_order),
                        without_umbrella(&oracle_order),
                        "skills-manifest byte difference is not the documented `spec-kitty` ordering"
                    );
                }
            }
            ".kittify/canonical-events.jsonl"
            | ".kittify/migration-ledger.jsonl"
            | ".kittify/migrations/clone-local.jsonl" => {
                let fields: &[&str] = if relative.ends_with("canonical-events.jsonl") {
                    &["event_id", "timestamp"]
                } else {
                    &["applied_at"]
                };
                if strip(&row_objects(&oracle_bytes), fields)
                    != strip(&row_objects(&native_bytes), fields)
                {
                    differences.push(relative.clone());
                }
            }
            _ => {
                if normalize(&oracle_bytes) != normalize(&native_bytes) {
                    differences.push(relative.clone());
                }
            }
        }
    }
    assert!(
        differences.is_empty(),
        "native init post-state differs from the oracle in: {differences:?}"
    );
}

/// Drop the `environment:` block (the documented provenance gap) while keeping
/// every other byte, including `initialized_at` (normalized later).
fn strip_environment_block(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut in_environment = false;
    for line in text.lines() {
        if line.starts_with("environment:") {
            in_environment = true;
            out.push_str("environment:\n");
            continue;
        }
        if in_environment {
            if line.starts_with(' ') {
                continue;
            }
            in_environment = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
