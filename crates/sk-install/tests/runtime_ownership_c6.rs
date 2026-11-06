//! C6 differential: the user-global runtime owner inventory and `.update.lock`.
//!
//! Oracle: `tests/fixtures/runtime_ownership_c6.json`, recorded by the pinned
//! Python CLI (`tests/fixtures/runtime_ownership_c6_probe.py`). The assertions
//! below compare literal bytes (the inventory text) and real observed node
//! state; nothing is normalized away, and the two non-deterministic facts
//! (mtimes, inodes) are asserted as *relations* over the actual run.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_install::migrations::completion_modern::global_runtime::{
    INVENTORY_FILE, NodeKind, UPDATE_LOCK_FILE, VERSION_LOCK_FILE, ensure_runtime_with, observe,
    plan_runtime_with,
};

const FIXTURE_REL: &str = "tests/fixtures/runtime_ownership_c6.json";

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_REL);
    let raw = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "the recorded Source runtime-ownership oracle {} is unavailable ({error}); \
             a missing oracle is a failure, never a pass",
            path.display()
        )
    });
    serde_json::from_slice(&raw).expect("recorded runtime-ownership oracle is JSON")
}

fn sandbox(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sk-c6-runtime-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(path.join("home")).expect("sandbox");
    path
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
}

fn tree(base: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(base)
                .expect("under base")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = std::fs::symlink_metadata(&path).expect("metadata");
            if metadata.file_type().is_symlink() {
                out.push((
                    relative,
                    format!(
                        "symlink:{}",
                        std::fs::read_link(&path)
                            .map(|target| target.display().to_string())
                            .unwrap_or_default()
                    ),
                ));
            } else if metadata.is_dir() {
                out.push((relative, "dir".to_string()));
                stack.push(path);
            } else {
                use std::os::unix::fs::PermissionsExt;
                let bytes = std::fs::read(&path).expect("bytes");
                out.push((
                    relative,
                    format!(
                        "file:{:o}:{}",
                        metadata.permissions().mode() & 0o7777,
                        sk_install::manifest::content_hash(&bytes)
                    ),
                ));
            }
        }
    }
    out.sort();
    out
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o7777
}

fn home(sandbox: &Path) -> PathBuf {
    sandbox.join("home").join(".kittify")
}

#[test]
fn fresh_provisioning_writes_the_recorded_owner_inventory_and_lock() {
    let oracle = fixture();
    let sandbox = sandbox("fresh");
    let user_home = sandbox.join("home");
    let runtime = home(&sandbox);
    std::fs::create_dir_all(&runtime).expect("runtime home");

    let version = oracle["version"].as_str().expect("recorded version");
    let report = ensure_runtime_with(&runtime, &user_home, version)
        .expect("provisions")
        .expect("package assets present");
    assert!(report.changed());
    assert_eq!(
        report.lock.as_deref(),
        Some(
            runtime
                .join("cache")
                .join(UPDATE_LOCK_FILE)
                .to_str()
                .expect("utf-8 path")
        )
    );

    let inventory_path = runtime.join("cache").join(INVENTORY_FILE);
    let written = std::fs::read_to_string(&inventory_path).expect("inventory written");
    // Byte-for-byte the recorded Source record: same key order, same entry
    // fields, same `mtime_ns: null`, same trailing newline.
    assert_eq!(
        written,
        oracle["inventory_text"]
            .as_str()
            .expect("recorded inventory"),
        "the native owner inventory differs from the recorded Source bytes"
    );
    let parsed: Value = serde_json::from_str(&written).expect("inventory JSON");
    assert_eq!(
        parsed["entries"].as_object().map(serde_json::Map::len),
        oracle["inventory_entry_count"]
            .as_u64()
            .map(|count| count as usize)
    );

    // The persistent lock: a real regular file, mode 0644, empty content — the
    // flock anchor, not a placeholder.
    let lock = runtime.join("cache").join(UPDATE_LOCK_FILE);
    let lock_state = observe(&lock);
    assert_eq!(lock_state.kind, NodeKind::File);
    assert_eq!(
        lock_state.sha256.as_deref(),
        oracle["lock"]["sha256"].as_str()
    );
    assert_eq!(
        lock_state.mode.map(u64::from),
        oracle["lock"]["mode"].as_u64()
    );
    assert!(
        lock_state.mtime_ns.is_some(),
        "the lock has no observed mtime"
    );
    assert_eq!(
        std::fs::read(&lock).expect("lock bytes").len() as u64,
        oracle["lock"]["size"].as_u64().expect("recorded lock size")
    );
    assert!(
        !parsed["entries"]
            .get("home/.kittify/cache/.update.lock")
            .is_some()
    );

    // The version stamp is the recorded value, and the managed trees carry the
    // recorded modes (the inventory above already binds every hash).
    assert_eq!(
        std::fs::read_to_string(runtime.join("cache").join(VERSION_LOCK_FILE))
            .expect("version stamp")
            .trim(),
        version
    );
    let mission = runtime.join("missions/software-dev");
    assert_eq!(observe(&mission).kind, NodeKind::Directory);
    #[cfg(unix)]
    assert_eq!(mode_of(&mission), 0o755);
    // The package ships only these mission trees (and no `AGENTS.md`), so the
    // rest are skipped exactly as the recorded inventory shows (no `audit`
    // entries, no `scripts` tree, no `AGENTS.md` entry).
    assert_eq!(
        report.skipped,
        vec![
            "missions/audit".to_string(),
            "missions/refactor".to_string(),
            "scripts".to_string(),
            "AGENTS.md".to_string(),
        ]
    );
}

#[test]
fn a_second_pass_is_a_no_op_including_mtimes() {
    let oracle = fixture();
    let sandbox = sandbox("warm");
    let user_home = sandbox.join("home");
    let runtime = home(&sandbox);
    std::fs::create_dir_all(&runtime).expect("runtime home");
    let version = oracle["version"].as_str().expect("recorded version");
    ensure_runtime_with(&runtime, &user_home, version)
        .expect("provisions")
        .expect("assets");
    let before = tree(&runtime);
    let inventory_before =
        std::fs::read_to_string(runtime.join("cache").join(INVENTORY_FILE)).expect("inventory");
    let lock_inode = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(runtime.join("cache").join(UPDATE_LOCK_FILE))
            .expect("lock")
            .ino()
    };
    let mtimes: Vec<(String, i64)> = before
        .iter()
        .filter(|(_, state)| state.starts_with("file:"))
        .map(|(relative, _)| {
            (
                relative.clone(),
                observe(&runtime.join(relative)).mtime_ns.expect("mtime"),
            )
        })
        .collect();

    let report = ensure_runtime_with(&runtime, &user_home, version)
        .expect("provisions again")
        .expect("assets");
    assert!(!report.changed(), "a warm pass rewrote {report:?}");
    assert_eq!(tree(&runtime), before, "a warm pass changed nodes");
    assert_eq!(
        std::fs::read_to_string(runtime.join("cache").join(INVENTORY_FILE)).expect("inventory"),
        inventory_before
    );
    for (relative, mtime) in mtimes {
        assert_eq!(
            observe(&runtime.join(&relative)).mtime_ns,
            Some(mtime),
            "{relative} was rewritten by a warm pass"
        );
    }
    let lock_inode_after = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(runtime.join("cache").join(UPDATE_LOCK_FILE))
            .expect("lock")
            .ino()
    };
    assert_eq!(
        lock_inode_after, lock_inode,
        "the flock anchor was replaced"
    );
    assert_eq!(oracle["warm_stable"].as_bool(), Some(true));
    assert_eq!(oracle["warm_inventory_identical"].as_bool(), Some(true));
}

#[test]
fn drift_operator_files_and_symlinks_are_handled_like_the_source() {
    let oracle = fixture();
    let sandbox = sandbox("mixed");
    let user_home = sandbox.join("home");
    let runtime = home(&sandbox);
    std::fs::create_dir_all(&runtime).expect("runtime home");
    let version = oracle["version"].as_str().expect("recorded version");
    ensure_runtime_with(&runtime, &user_home, version)
        .expect("provisions")
        .expect("assets");
    let managed = runtime.join("missions/software-dev/mission.yaml");
    let pristine = std::fs::read(&managed).expect("pristine bytes");
    write(&managed, "operator edit\n");
    write(
        &runtime.join("missions/software-dev/operator.md"),
        "keep me\n",
    );
    write(&runtime.join("missions/custom/mine.md"), "custom\n");
    write(&runtime.join("config.yaml"), "project: keep\n");
    write(&runtime.join("cache/operator.txt"), "cache\n");
    let linked = runtime.join("missions/software-dev/expected-artifacts.yaml");
    let outside = sandbox.join("outside-artifacts.yaml");
    write(&outside, "linked\n");
    std::fs::remove_file(&linked).expect("remove");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &linked).expect("symlink");

    let report = ensure_runtime_with(&runtime, &user_home, version)
        .expect("provisions")
        .expect("assets");
    // The shipped file is refreshed from the package (a managed tree replaces
    // regular files it ships), while every unowned node is left exactly alone.
    assert_eq!(std::fs::read(&managed).expect("managed bytes"), pristine);
    assert_eq!(
        std::fs::read_to_string(runtime.join("missions/software-dev/operator.md"))
            .expect("operator file"),
        "keep me\n"
    );
    assert_eq!(
        std::fs::read_to_string(runtime.join("missions/custom/mine.md")).expect("custom"),
        "custom\n"
    );
    assert_eq!(
        std::fs::read_to_string(runtime.join("config.yaml")).expect("config"),
        "project: keep\n"
    );
    assert_eq!(
        std::fs::read_to_string(runtime.join("cache/operator.txt")).expect("cache"),
        "cache\n"
    );
    // A symlink at a managed destination is preserved, never written through.
    assert!(linked.is_symlink(), "the managed symlink was replaced");
    assert_eq!(
        std::fs::read_to_string(&outside).expect("outside"),
        "linked\n"
    );
    assert!(
        report.preserved.iter().any(|entry| entry
            .starts_with("home/.kittify/missions/software-dev/expected-artifacts.yaml")),
        "the preserved symlink was not reported: {:?}",
        report.preserved
    );
    assert_eq!(
        oracle["mixed"]["managed_file_restored"].as_bool(),
        Some(true)
    );
    assert_eq!(oracle["mixed"]["operator_file_kept"].as_bool(), Some(true));
    assert_eq!(oracle["mixed"]["symlink_preserved"].as_bool(), Some(true));
}

#[test]
fn recorded_stale_nodes_are_pruned_and_edited_ones_preserved() {
    let oracle = fixture();
    let sandbox = sandbox("stale");
    let user_home = sandbox.join("home");
    let runtime = home(&sandbox);
    std::fs::create_dir_all(&runtime).expect("runtime home");
    let version = oracle["version"].as_str().expect("recorded version");
    ensure_runtime_with(&runtime, &user_home, version)
        .expect("provisions")
        .expect("assets");
    // Simulate the previous release's record: two managed files it shipped and
    // this package no longer does. One is untouched, one was edited by hand.
    let inventory_path = runtime.join("cache").join(INVENTORY_FILE);
    let mut inventory: Value =
        serde_json::from_str(&std::fs::read_to_string(&inventory_path).expect("inventory"))
            .expect("json");
    let obsolete: &[u8] = b"old release\n";
    let edited: &[u8] = b"old release edited\n";
    write(
        &runtime.join("missions/software-dev/obsolete.md"),
        "old release\n",
    );
    write(
        &runtime.join("missions/software-dev/edited.md"),
        "old release edited\n",
    );
    let entries = inventory["entries"].as_object_mut().expect("entries");
    for (relative, bytes) in [
        ("home/.kittify/missions/software-dev/obsolete.md", obsolete),
        ("home/.kittify/missions/software-dev/edited.md", edited),
    ] {
        entries.insert(
            relative.to_string(),
            serde_json::json!({
                "kind": "file",
                "mode": 420,
                "mtime_ns": null,
                "sha256": sk_install::manifest::content_hash(bytes),
                "target": null,
            }),
        );
    }
    std::fs::write(
        &inventory_path,
        serde_json::to_string_pretty(&inventory).expect("render") + "\n",
    )
    .expect("write inventory");
    write(
        &runtime.join("missions/software-dev/edited.md"),
        "operator drift\n",
    );

    let report = ensure_runtime_with(&runtime, &user_home, version)
        .expect("provisions")
        .expect("assets");
    assert!(
        !runtime.join("missions/software-dev/obsolete.md").exists(),
        "an unchanged recorded stale node was not pruned"
    );
    assert!(
        report
            .pruned
            .iter()
            .any(|path| path.ends_with("missions/software-dev/obsolete.md")),
        "the prune was not reported"
    );
    assert_eq!(
        std::fs::read_to_string(runtime.join("missions/software-dev/edited.md"))
            .expect("edited recorded file"),
        "operator drift\n",
        "an edited recorded node was destroyed"
    );
    assert_eq!(
        oracle["stale"]["unchanged_recorded_pruned"].as_bool(),
        Some(true)
    );
    assert_eq!(
        oracle["stale"]["drifted_recorded_preserved"].as_bool(),
        Some(true)
    );
}

#[test]
fn a_corrupt_inventory_is_refused_and_never_reset() {
    let oracle = fixture();
    let sandbox = sandbox("corrupt");
    let user_home = sandbox.join("home");
    let runtime = home(&sandbox);
    std::fs::create_dir_all(runtime.join("cache")).expect("cache");
    let inventory_path = runtime.join("cache").join(INVENTORY_FILE);
    std::fs::write(&inventory_path, "{not json\n").expect("corrupt inventory");
    let version = oracle["version"].as_str().expect("recorded version");
    let error = ensure_runtime_with(&runtime, &user_home, version)
        .expect_err("a corrupt inventory must be refused");
    assert_eq!(
        error.code(),
        "manifest_invalid_entry",
        "unexpected refusal: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(&inventory_path).expect("kept bytes"),
        "{not json\n",
        "the corrupt inventory was overwritten"
    );
    assert_eq!(oracle["corrupt"]["outcome"].as_str(), Some("error"));
}

#[test]
fn a_managed_tree_replaced_by_a_regular_file_is_preserved() {
    let oracle = fixture();
    let sandbox = sandbox("collision");
    let user_home = sandbox.join("home");
    let runtime = home(&sandbox);
    std::fs::create_dir_all(&runtime).expect("runtime home");
    write(&runtime.join("missions/software-dev"), "not a directory\n");
    let version = oracle["version"].as_str().expect("recorded version");
    // The preview (read-only) describes the same preservation the run performs.
    let plan = plan_runtime_with(&runtime, &user_home, version)
        .expect("plans")
        .expect("assets");
    assert!(
        plan.changed(),
        "the preview must describe the pending inventory write"
    );
    let report = ensure_runtime_with(&runtime, &user_home, version)
        .expect("provisions")
        .expect("assets");
    assert_eq!(
        std::fs::read_to_string(runtime.join("missions/software-dev")).expect("kept file"),
        "not a directory\n",
        "a regular file at a managed tree destination was replaced"
    );
    assert!(
        report
            .preserved
            .iter()
            .any(|entry| entry.contains("missions/software-dev")),
        "the preserved collision was not reported: {:?}",
        report.preserved
    );
    // The sibling managed trees are still provisioned.
    assert!(runtime.join("missions/research/mission.yaml").is_file());
    assert_eq!(oracle["collision"]["scripts_kind"].as_str(), Some("file"));
}
