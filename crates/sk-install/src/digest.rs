//! Pack tree digests, byte-compatible with the Python pin verifier.
//!
//! Two independent digests must both match a pin before a tree is used:
//!
//! * [`clean_tree_digest`] — the *declared* file set only (`pack.yaml`, the
//!   tier's layout files, and the recognised artifact directories), with CRLF/CR
//!   normalized to LF. Build noise (`__pycache__`, `*.pyc`) is excluded by
//!   construction, so a `git archive` export and a working tree digest
//!   identically.
//! * [`full_tree_digest`] — every file under the root, raw bytes, including
//!   `missions/`, `templates/`, `pack-manifest.yaml` and dotfiles, with only
//!   bytecode exempt. Symlinks and special files are refused.
//!
//! Byte format (both): for each file, in sorted relative-POSIX order,
//! `u64be(rel.len) || rel || u64be(content.len) || content`. This mirrors
//! `charter/offering/packs/pack_clean_digest.py` and
//! `charter/offering/packs/pack_manifest.py::snapshot_sha256` exactly; the crate
//! tests pin the values against the Python oracle.
//!
//! The recognised artifact directories and layout constants are taken from the
//! single `sk-charter` taxonomy, not re-spelled here.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use sk_charter::{Limits, PackTier};

use crate::error::InstallError;

/// The only supported clean-tree digest algorithm token.
pub const TREE_DIGEST_ALGORITHM: &str = "clean-tree-sha256";

/// Authored pack descriptor file name.
pub const PACK_DESCRIPTOR: &str = "pack.yaml";

/// Built-in DRG shard glob (fragments live at the pack root).
const BUILT_IN_GRAPH_SUFFIX: &str = ".graph.yaml";
/// Org-pack layout: charter file and DRG fragment.
const ORG_CHARTER: &str = "org-charter.yaml";
const DRG_FRAGMENT: &str = "drg/fragment.yaml";
/// Project-pack layout.
const PROJECT_GRAPH: &str = "graph.yaml";

/// Directory names never part of a pack's declared content.
const BYTECODE_DIR: &str = "__pycache__";

/// The recognised artifact directories a clean-tree digest enumerates.
///
/// Mirrors `charter/offering/artifact_kinds.py` → `has_layered_repository`
/// plurals plus the `drg` layout leaf (every kind except `templates` and
/// `anti_patterns`). `sk-charter` owns the kind taxonomy but does not yet expose
/// this derived set, so it is declared here and pinned by a crate test; when
/// `sk-charter` grows the accessor this constant is deleted in favour of it.
pub const RECOGNISED_ARTIFACT_DIRS: [&str; 12] = [
    "directives",
    "tactics",
    "styleguides",
    "toolguides",
    "paradigms",
    "procedures",
    "agent_profiles",
    "mission_step_contracts",
    "assets",
    "glossary_packs",
    "skills",
    "drg",
];

/// Compute the clean-tree digest of `pack_root` at `tier`.
///
/// Fails closed when `pack.yaml` is missing or schema-invalid (validated through
/// `sk-charter`, the single descriptor authority) and when a declared file is a
/// symlink.
pub fn clean_tree_digest(pack_root: &Path, tier: PackTier) -> Result<String, InstallError> {
    let descriptor = pack_root.join(PACK_DESCRIPTOR);
    if !descriptor.is_file() {
        return Err(InstallError::PinInvalid {
            path: descriptor,
            detail: "no pack.yaml descriptor".to_string(),
        });
    }
    sk_charter::load_pack_descriptor(tier, pack_root, &Limits::default()).map_err(|error| {
        InstallError::PinInvalid {
            path: pack_root.join(PACK_DESCRIPTOR),
            detail: error.to_string(),
        }
    })?;

    let mut files: Vec<PathBuf> = vec![descriptor];
    files.extend(layout_files(pack_root, tier)?);
    for name in recognised_artifact_dirs() {
        let directory = pack_root.join(&name);
        if directory.is_dir() {
            files.extend(artifact_dir_files(&directory)?);
        }
    }

    files.sort_by_key(|candidate| relative_posix(pack_root, candidate));
    files.dedup();

    let mut hasher = Sha256::new();
    for file in files {
        let relative = relative_posix(pack_root, &file);
        let raw = read_file(pack_root, &file)?;
        let content = lf_normalized(&raw);
        hasher.update(u64_bytes(relative.len() as u64));
        hasher.update(relative.as_bytes());
        hasher.update(u64_bytes(content.len() as u64));
        hasher.update(&content);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Compute the full-tree digest of a pack root: every file, raw bytes.
pub fn full_tree_digest(pack_root: &Path) -> Result<String, InstallError> {
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    collect_tree(pack_root, pack_root, &mut entries)?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    for (relative, path) in entries {
        if is_bytecode(&relative) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|source| InstallError::io("inspect", &path, source))?;
        if metadata.file_type().is_symlink() {
            return Err(InstallError::PackSymlink { path });
        }
        if metadata.is_dir() {
            continue;
        }
        if !metadata.is_file() {
            return Err(InstallError::PackSpecialFile { path });
        }
        let mut file =
            std::fs::File::open(&path).map_err(|source| InstallError::io("open", &path, source))?;
        let length = metadata.len();
        hasher.update(u64_bytes(relative.len() as u64));
        hasher.update(relative.as_bytes());
        hasher.update(u64_bytes(length));
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|source| InstallError::io("read", &path, source))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Refuse any symlink or special file anywhere under `root`.
pub fn refuse_symlinks(root: &Path) -> Result<(), InstallError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current)
            .map_err(|source| InstallError::io("read directory", &current, source))?;
        for entry in entries {
            let entry = entry
                .map_err(|source| InstallError::io("read directory entry", &current, source))?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|source| InstallError::io("inspect", &path, source))?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(InstallError::PackSymlink { path });
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if !file_type.is_file() {
                return Err(InstallError::PackSpecialFile { path });
            }
        }
    }
    Ok(())
}

/// Normalize CRLF and lone CR to LF (cross-platform-stable digest input).
pub fn lf_normalized(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                output.push(b'\n');
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
            }
            byte => output.push(byte),
        }
        index += 1;
    }
    output
}

/// The recognised artifact directory names, as owned strings for iteration.
fn recognised_artifact_dirs() -> Vec<String> {
    RECOGNISED_ARTIFACT_DIRS
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

fn layout_files(pack_root: &Path, tier: PackTier) -> Result<Vec<PathBuf>, InstallError> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    match tier {
        PackTier::BuiltIn => {
            let entries = std::fs::read_dir(pack_root)
                .map_err(|source| InstallError::io("read directory", pack_root, source))?;
            let mut shards: Vec<PathBuf> = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|source| {
                    InstallError::io("read directory entry", pack_root, source)
                })?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(BUILT_IN_GRAPH_SUFFIX) {
                    shards.push(entry.path());
                }
            }
            shards.sort();
            candidates.extend(shards);
        }
        PackTier::Org => {
            candidates.push(pack_root.join(ORG_CHARTER));
            candidates.push(pack_root.join(DRG_FRAGMENT));
        }
        PackTier::Project => candidates.push(pack_root.join(PROJECT_GRAPH)),
    }
    Ok(candidates
        .into_iter()
        .filter(|path| {
            std::fs::symlink_metadata(path)
                .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
                .unwrap_or(false)
        })
        .collect())
}

fn artifact_dir_files(directory: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let mut collected = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current)
            .map_err(|source| InstallError::io("read directory", &current, source))?;
        for entry in entries {
            let entry = entry
                .map_err(|source| InstallError::io("read directory entry", &current, source))?;
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|source| InstallError::io("inspect", &path, source))?;
            let file_type = metadata.file_type();
            if file_type.is_dir() {
                if name == BYTECODE_DIR {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file()
                && !file_type.is_symlink()
                && !name.ends_with(".pyc")
                && !name.ends_with(".pyo")
            {
                collected.push(path);
            }
        }
    }
    Ok(collected)
}

fn collect_tree(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(String, PathBuf)>,
) -> Result<(), InstallError> {
    let read_dir = std::fs::read_dir(current)
        .map_err(|source| InstallError::io("read directory", current, source))?;
    for entry in read_dir {
        let entry =
            entry.map_err(|source| InstallError::io("read directory entry", current, source))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|source| InstallError::io("inspect", &path, source))?;
        let relative = relative_posix(root, &path);
        if metadata.file_type().is_dir() {
            entries.push((relative, path.clone()));
            collect_tree(root, &path, entries)?;
        } else {
            entries.push((relative, path));
        }
    }
    Ok(())
}

fn relative_posix(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn read_file(root: &Path, path: &Path) -> Result<Vec<u8>, InstallError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|source| InstallError::io("inspect", path, source))?;
    if metadata.file_type().is_symlink() {
        return Err(InstallError::PackSymlink {
            path: path.to_path_buf(),
        });
    }
    let _ = root;
    std::fs::read(path).map_err(|source| InstallError::io("read", path, source))
}

fn is_bytecode(relative: &str) -> bool {
    relative.split('/').any(|part| part == BYTECODE_DIR) || relative.ends_with(".pyc")
}

fn u64_bytes(value: u64) -> [u8; 8] {
    value.to_be_bytes()
}
