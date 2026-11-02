//! Shared, dependency-free fixture helpers for the sk-charter acceptance tests.
//!
//! Fixtures are written as real files in a temporary directory; the crate under
//! test only ever reads them. `tree_snapshot` lets a test prove that inspection
//! did not rewrite user-authored YAML or Markdown.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write `contents` to `path`, creating parents.
pub fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent");
    }
    let mut file = fs::File::create(path).expect("create fixture file");
    file.write_all(contents.as_bytes())
        .expect("write fixture file");
}

/// Create an empty directory (and parents).
pub fn mkdir(path: &Path) {
    fs::create_dir_all(path).expect("create fixture directory");
}

/// Recursively snapshot every regular file's bytes under `root`.
///
/// Symlinks are recorded but not followed, so a symlink fixture cannot make the
/// snapshot escape the temporary directory.
pub fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut snapshot = BTreeMap::new();
    walk(root, root, &mut snapshot);
    snapshot
}

fn walk(root: &Path, dir: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => panic!("read {}: {error}", dir.display()),
    };
    for entry in entries {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        let relative = path.strip_prefix(root).expect("relative").to_path_buf();
        let file_type = entry.file_type().expect("file type");
        if file_type.is_symlink() {
            let target = fs::read_link(&path).expect("read link");
            snapshot.insert(
                relative,
                format!("symlink:{}", target.display()).into_bytes(),
            );
        } else if file_type.is_dir() {
            walk(root, &path, snapshot);
        } else if file_type.is_file() {
            snapshot.insert(relative, fs::read(&path).expect("read fixture file"));
        }
    }
}

/// A minimal, schema-valid `pack.yaml` for `tier`.
pub fn pack_yaml(name: &str, level: &str, pack_id: &str, version: Option<&str>) -> String {
    let mut body = format!("pack_id: {pack_id}\nname: {name}\npack_level: {level}\n");
    if let Some(version) = version {
        body.push_str(&format!("pack_version: {version}\n"));
    }
    body
}

/// The fixed canonical built-in pack ULID used by the Python fixtures.
pub const BUILTIN_PACK_ID: &str = "01ARWG13C000000000000000FG";

/// Write one `*.asset.yaml` sidecar manifest.
pub fn asset_yaml(id: &str, mime: &str, path: &str) -> String {
    format!("id: {id}\nmime: {mime}\npath: {path}\n")
}

/// Create a directory symlink. Unix-only fixture tests are gated by the caller.
#[cfg(unix)]
pub fn symlink_dir(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create symlink");
}

/// Create a file symlink. Unix-only fixture tests are gated by the caller.
#[cfg(unix)]
pub fn symlink_file(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create symlink");
}
