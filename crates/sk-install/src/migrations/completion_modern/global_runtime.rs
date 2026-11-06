//! User-global runtime provisioning (`~/.kittify/`) — owner manifest and lock.
//!
//! Source: `src/specify_cli/runtime/bootstrap.py::{assess_runtime,ensure_runtime}`,
//! `src/specify_cli/runtime/asset_preparation.py` (the owner inventory + flock
//! protocol) and `src/specify_cli/runtime/merge.py`'s managed-asset policy.
//!
//! The pinned CLI materialises the package's managed assets into the user-global
//! runtime home on `spec-kitty migrate` (and on every startup), and persists the
//! *ownership record* that makes the next pass provable
//! (`cache/runtime_bootstrap-assets.json`) under one persistent flock anchor
//! (`cache/.update.lock`).
//!
//! Policy, from the recorded Source oracle
//! (`/tmp/sk-c6/runtime-ownership-record-c6.json`):
//!
//! * **managed directories** (`missions/<built-in type>` for every shipped
//!   mission, plus `scripts`) are OVERLAID from the package: every package node
//!   is written; a recorded path the new tree no longer ships is pruned **only
//!   while its bytes still match** the record — an edited node is preserved, and
//!   a directory with preserved content stays. Operator files inside a managed
//!   tree are never visited, so they survive (the C4 overlay fix, retained);
//! * **managed files** (`AGENTS.md`) are written the same way when the package
//!   ships them (the pinned `packs/built-in` does not, and the Source pass then
//!   records nothing);
//! * everything else — `config.yaml`, `missions/custom/`, `cache/`, operator
//!   additions, a foreign symlink — is **never touched**;
//! * a managed destination that is a symlink is PRESERVED, never replaced or
//!   written through; a directory replaced by a non-directory is preserved
//!   ("Directory replacement requires enumerated ownership");
//! * `cache/version.lock` records the running CLI version, which is what
//!   `spec-kitty doctor` compares;
//! * the inventory records `kind`/`mode`/`sha256`/`target` per owned node and
//!   never mints a value it did not observe (`mtime_ns` stays `null`, exactly
//!   like the recorded Source record); the persistent lock node is created by
//!   the flock acquisition and is not an inventory entry.
//!
//! Writes are atomic and symlink-refusing through the kernel writer, the whole
//! pass is serialized by the kernel [`sk_kernel::FileLock`] on
//! `cache/.update.lock`, and the inventory is written last so a partial pass can
//! never certify work.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::error::InstallError;
use crate::migrations::completion_legacy::shared;

/// The Source owner key for the runtime bootstrap assets.
pub const OWNER: &str = "runtime_bootstrap";
/// The owner inventory file name (`<owner>-assets.json`).
pub const INVENTORY_FILE: &str = "runtime_bootstrap-assets.json";
/// The persistent flock anchor (`cache/.update.lock`).
pub const UPDATE_LOCK_FILE: &str = ".update.lock";
/// The version stamp (`cache/version.lock`).
pub const VERSION_LOCK_FILE: &str = "version.lock";
/// The one inventory schema version this build reads and writes.
pub const INVENTORY_SCHEMA_VERSION: u64 = 1;
/// The default bounded wait for the owner lock.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(30);
/// The mode a managed directory is materialised with (`tree(managed_tree=True)`).
const MANAGED_DIR_MODE: u32 = 0o755;

/// Package-managed runtime directories, relative to the runtime home.
///
/// `missions/custom/` is deliberately absent: it is user-owned.
pub const MANAGED_DIRS: [&str; 7] = [
    "missions/software-dev",
    "missions/research",
    "missions/documentation",
    "missions/plan",
    "missions/audit",
    "missions/refactor",
    "scripts",
];

/// Package-managed runtime files, relative to the runtime home.
pub const MANAGED_FILES: [&str; 1] = ["AGENTS.md"];

/// The node kinds the inventory and the observations use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    /// No node at the path.
    Absent,
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link (its literal target is recorded, never followed).
    Symlink,
}

impl NodeKind {
    /// The inventory spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::File => "file",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "absent" => Some(Self::Absent),
            "file" => Some(Self::File),
            "directory" => Some(Self::Directory),
            "symlink" => Some(Self::Symlink),
            _ => None,
        }
    }
}

/// One exact node observation / inventory record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeState {
    /// The node kind.
    pub kind: NodeKind,
    /// The file content digest (`file` only).
    pub sha256: Option<String>,
    /// The literal symlink target (`symlink` only).
    pub target: Option<String>,
    /// The permission bits.
    pub mode: Option<u32>,
    /// The observed modification time in nanoseconds, when observed.
    pub mtime_ns: Option<i64>,
}

impl Default for NodeState {
    fn default() -> Self {
        Self::absent()
    }
}

impl NodeState {
    /// The absent state.
    pub fn absent() -> Self {
        Self {
            kind: NodeKind::Absent,
            sha256: None,
            target: None,
            mode: None,
            mtime_ns: None,
        }
    }

    /// A desired directory state (no mtime: a desired state is not an observation).
    pub fn directory(mode: u32) -> Self {
        Self {
            kind: NodeKind::Directory,
            sha256: None,
            target: None,
            mode: Some(mode),
            mtime_ns: None,
        }
    }

    /// A desired file state (no mtime: a desired state is not an observation).
    pub fn file(sha256: String, mode: u32) -> Self {
        Self {
            kind: NodeKind::File,
            sha256: Some(sha256),
            target: None,
            mode: Some(mode),
            mtime_ns: None,
        }
    }

    /// The ownership comparison Source uses: kind, content and link target.
    fn ownership_matches(&self, other: &Self) -> bool {
        self.kind == other.kind && self.sha256 == other.sha256 && self.target == other.target
    }

    /// The content comparison Source uses for "already matches".
    fn content_matches(&self, other: &Self) -> bool {
        self.kind == other.kind && self.sha256 == other.sha256
    }

    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert(
            "kind".to_string(),
            Value::String(self.kind.as_str().to_string()),
        );
        map.insert(
            "mode".to_string(),
            self.mode.map_or(Value::Null, Value::from),
        );
        map.insert(
            "mtime_ns".to_string(),
            self.mtime_ns.map_or(Value::Null, Value::from),
        );
        map.insert(
            "sha256".to_string(),
            self.sha256.clone().map_or(Value::Null, Value::String),
        );
        map.insert(
            "target".to_string(),
            self.target.clone().map_or(Value::Null, Value::String),
        );
        Value::Object(map)
    }

    fn from_value(path: &Path, value: &Value) -> Result<Self, InstallError> {
        let object = value
            .as_object()
            .ok_or_else(|| inventory_invalid(path, "entry is not an object"))?;
        if let Some(key) = object.keys().find(|key| {
            !matches!(
                key.as_str(),
                "kind" | "sha256" | "target" | "mode" | "mtime_ns"
            )
        }) {
            return Err(inventory_invalid(
                path,
                format!("unexpected inventory state field {key:?}"),
            ));
        }
        let kind_text = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| inventory_invalid(path, "entry has no string kind"))?;
        let kind = NodeKind::parse(kind_text)
            .ok_or_else(|| inventory_invalid(path, format!("unknown node kind {kind_text:?}")))?;
        let mode = optional_u32(path, value, "mode")?;
        let mtime_ns = value
            .get("mtime_ns")
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_i64()
                    .ok_or_else(|| inventory_invalid(path, "mtime_ns is not an integer"))
            })
            .transpose()?;
        let sha256 = optional_string(path, value, "sha256")?;
        let target = optional_string(path, value, "target")?;
        match kind {
            NodeKind::Absent => {
                if sha256.is_some() || target.is_some() || mode.is_some() || mtime_ns.is_some() {
                    return Err(inventory_invalid(path, "an absent record carries no state"));
                }
            }
            NodeKind::File => {
                let hash = sha256
                    .as_deref()
                    .ok_or_else(|| inventory_invalid(path, "a file record needs sha256"))?;
                if hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(inventory_invalid(path, "sha256 is not 64 hex digits"));
                }
                if target.is_some() {
                    return Err(inventory_invalid(path, "a file record carries no target"));
                }
                if mode.is_none() {
                    return Err(inventory_invalid(path, "a file record needs a mode"));
                }
            }
            NodeKind::Symlink => {
                if mode.is_none() {
                    return Err(inventory_invalid(path, "a symlink record needs a mode"));
                }
                if target.is_none() {
                    return Err(inventory_invalid(path, "a symlink record needs a target"));
                }
                if sha256.is_some() {
                    return Err(inventory_invalid(
                        path,
                        "a symlink record carries no sha256",
                    ));
                }
            }
            NodeKind::Directory => {
                if sha256.is_some() || target.is_some() {
                    return Err(inventory_invalid(
                        path,
                        "a directory record carries neither sha256 nor target",
                    ));
                }
                if mode.is_none() {
                    return Err(inventory_invalid(path, "a directory record needs a mode"));
                }
            }
        }
        Ok(Self {
            kind,
            sha256,
            target,
            mode,
            mtime_ns,
        })
    }
}

fn optional_u32(path: &Path, value: &Value, key: &str) -> Result<Option<u32>, InstallError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|raw| u32::try_from(raw).ok())
            .filter(|raw| *raw <= 0o7777)
            .map(Some)
            .ok_or_else(|| inventory_invalid(path, format!("{key} is not a permission mask"))),
    }
}

fn optional_string(path: &Path, value: &Value, key: &str) -> Result<Option<String>, InstallError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(inventory_invalid(path, format!("{key} is not a string"))),
    }
}

fn inventory_invalid(path: &Path, detail: impl Into<String>) -> InstallError {
    InstallError::ManifestInvalidEntry {
        path: path.to_path_buf(),
        detail: detail.into(),
    }
}

/// The owner inventory (`cache/runtime_bootstrap-assets.json`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OwnerInventory {
    /// Every recorded owner-relative POSIX path.
    pub entries: BTreeMap<String, NodeState>,
}

impl OwnerInventory {
    /// Load the inventory; `Ok(None)` when it does not exist.
    ///
    /// A present but corrupt record is a refusal, never a silent reset: the
    /// recorded Source oracle keeps the corrupt bytes and fails the pass.
    pub fn load(path: &Path) -> Result<Option<Self>, InstallError> {
        let raw = match std::fs::read(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(InstallError::io("read", path, source)),
        };
        let value: Value = serde_json::from_slice(&raw)
            .map_err(|error| inventory_invalid(path, format!("not valid JSON: {error}")))?;
        let schema = value
            .get("schema_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| inventory_invalid(path, "no integer schema_version"))?;
        if schema != INVENTORY_SCHEMA_VERSION {
            return Err(InstallError::ManifestUnsupportedVersion {
                path: path.to_path_buf(),
                found: schema.to_string(),
            });
        }
        let entries = value
            .get("entries")
            .and_then(Value::as_object)
            .ok_or_else(|| inventory_invalid(path, "no entries object"))?;
        let mut recorded = BTreeMap::new();
        for (relative, record) in entries {
            validate_relative(relative).map_err(|detail| inventory_invalid(path, detail))?;
            recorded.insert(relative.clone(), NodeState::from_value(path, record)?);
        }
        Ok(Some(Self { entries: recorded }))
    }

    /// Render the Source record shape (`json.dumps(sort_keys=True, indent=2)` + `\n`).
    pub fn render(&self) -> String {
        let mut entries = Map::new();
        for (relative, state) in &self.entries {
            entries.insert(relative.clone(), state.to_value());
        }
        let mut root = Map::new();
        root.insert("entries".to_string(), Value::Object(entries));
        root.insert(
            "schema_version".to_string(),
            Value::from(INVENTORY_SCHEMA_VERSION),
        );
        let mut text =
            serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        text
    }
}

/// Reject an inventory key that could escape the owner root.
fn validate_relative(relative: &str) -> Result<(), String> {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative.contains('\\')
        || relative.contains(':')
        || relative
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(format!("unconfined inventory key {relative:?}"));
    }
    Ok(())
}

/// What one planned effect does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectAction {
    /// Create a node that is absent.
    Create,
    /// Replace the node's content.
    Update,
    /// Change only the permission bits.
    Chmod,
    /// Remove a recorded node.
    Delete,
    /// Leave the node exactly as it is.
    Preserve,
}

impl EffectAction {
    /// The plan-json spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Chmod => "chmod",
            Self::Delete => "delete",
            Self::Preserve => "preserve",
        }
    }
}

/// One planned write on the runtime home.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeEffect {
    /// Owner-relative POSIX path.
    pub path: String,
    /// What the pass does.
    pub action: EffectAction,
    /// The observed state before the pass.
    pub before: NodeState,
    /// The state after the pass (a preserve carries `before`).
    pub after: NodeState,
    /// Why, in operator terms.
    pub reason: String,
}

/// The whole planned pass (read-only).
#[derive(Clone, Debug, Default)]
pub struct RuntimePlan {
    /// The runtime home.
    pub home: PathBuf,
    /// The owner operation root (inventory keys are relative to it).
    pub root: PathBuf,
    /// The effects, in application order (inventory and version stamp last).
    pub effects: Vec<RuntimeEffect>,
    /// The source path each write takes its bytes from.
    pub sources: BTreeMap<String, PathBuf>,
    /// The inventory as it stands before the pass.
    pub previous: OwnerInventory,
    /// The inventory the pass would persist.
    pub entries: OwnerInventory,
    /// Nodes preserved rather than written (with the Source reason).
    pub preserved: Vec<(String, String)>,
    /// Non-fatal notes (orphan staging directories).
    pub warnings: Vec<String>,
    /// The version stamp the pass would write, when it differs.
    pub version_lock: Option<String>,
    /// Whether the owner inventory would be rewritten.
    pub inventory_changed: bool,
    /// Managed assets whose package source is absent (skipped, not failed).
    pub skipped: Vec<String>,
}

impl RuntimePlan {
    /// Whether applying the plan would change anything on disk.
    pub fn changed(&self) -> bool {
        self.inventory_changed
            || self.version_lock.is_some()
            || self
                .effects
                .iter()
                .any(|effect| effect.action != EffectAction::Preserve)
    }
}

/// What one provisioning pass did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeProvisionReport {
    /// The runtime home that was provisioned.
    pub home: PathBuf,
    /// Managed directories written (runtime-relative POSIX).
    pub directories: Vec<String>,
    /// Managed files written (runtime-relative POSIX).
    pub files: Vec<String>,
    /// Recorded nodes pruned.
    pub pruned: Vec<String>,
    /// Nodes preserved rather than written.
    pub preserved: Vec<String>,
    /// `cache/version.lock` content written, when it changed.
    pub version_lock: Option<String>,
    /// `cache/runtime_bootstrap-assets.json` written, when it changed.
    pub inventory: Option<String>,
    /// The persistent lock anchor holding the pass.
    pub lock: Option<String>,
    /// Managed assets the package does not ship (skipped, not failed).
    pub skipped: Vec<String>,
    /// Non-fatal notes.
    pub warnings: Vec<String>,
}

impl RuntimeProvisionReport {
    /// Whether the pass wrote anything.
    pub fn changed(&self) -> bool {
        !self.directories.is_empty()
            || !self.files.is_empty()
            || !self.pruned.is_empty()
            || self.version_lock.is_some()
            || self.inventory.is_some()
    }
}

/// The package asset root (`packs/built-in/missions`) when available.
fn package_missions_root() -> Option<PathBuf> {
    shared::package_asset_root()
}

fn read_version_lock(home: &Path) -> Option<String> {
    std::fs::read_to_string(home.join("cache").join(VERSION_LOCK_FILE)).ok()
}

/// The owner operation root: `commonpath((HOME, home)).parent`.
///
/// Source `global_asset_root`: the reporting anchor covers the resolved global
/// destination, so inventory keys are relative to the directory ABOVE the home
/// folder. Reproduced from the observation, not guessed.
fn owner_root(home: &Path, user_home: &Path) -> PathBuf {
    let home_abs = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let user_home = user_home
        .canonicalize()
        .unwrap_or_else(|_| user_home.to_path_buf());
    let mut common = PathBuf::new();
    for (left, right) in user_home.components().zip(home_abs.components()) {
        if left != right {
            break;
        }
        common.push(left);
    }
    if common.as_os_str().is_empty() {
        common = home_abs
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home_abs.clone());
    }
    common.parent().map(Path::to_path_buf).unwrap_or(common)
}

fn relative_to_root(root: &Path, path: &Path) -> Result<String, InstallError> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| InstallError::PathEscape {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
        })
}

/// Observe one node without following it.
pub fn observe(path: &Path) -> NodeState {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return NodeState::absent(),
    };
    let file_type = metadata.file_type();
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode() & 0o7777)
    };
    #[cfg(not(unix))]
    let mode: Option<u32> = None;
    #[cfg(unix)]
    let mtime_ns = {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec().rem_euclid(1_000_000_000))
    };
    #[cfg(not(unix))]
    let mtime_ns: Option<i64> = None;
    if file_type.is_symlink() {
        return NodeState {
            kind: NodeKind::Symlink,
            sha256: None,
            target: std::fs::read_link(path)
                .ok()
                .map(|target| target.display().to_string()),
            mode,
            mtime_ns,
        };
    }
    if file_type.is_dir() {
        return NodeState {
            kind: NodeKind::Directory,
            sha256: None,
            target: None,
            mode,
            mtime_ns,
        };
    }
    NodeState {
        kind: NodeKind::File,
        sha256: std::fs::read(path)
            .ok()
            .map(|bytes| crate::manifest::content_hash(&bytes)),
        target: None,
        mode,
        mtime_ns,
    }
}

/// Read-only plan for one provisioning pass (no lock, no writes).
pub fn plan_runtime(home: &Path, cli_version: &str) -> Result<Option<RuntimePlan>, InstallError> {
    let user_home = sk_kernel::paths::home_directory().unwrap_or_else(|_| home.to_path_buf());
    plan_runtime_with(home, &user_home, cli_version)
}

/// [`plan_runtime`] with an explicit user home (the owner-anchor input).
///
/// The anchor is `commonpath((user_home, home)).parent`, so a caller that
/// resolves a different global runtime root supplies the home it resolved it
/// against instead of inheriting a test process's environment.
pub fn plan_runtime_with(
    home: &Path,
    user_home: &Path,
    cli_version: &str,
) -> Result<Option<RuntimePlan>, InstallError> {
    let Some(assets) = package_missions_root().filter(|assets| assets.is_dir()) else {
        return Ok(None);
    };
    let package_root = assets.parent().unwrap_or(&assets).to_path_buf();
    let root = owner_root(home, user_home);
    let inventory_path = home.join("cache").join(INVENTORY_FILE);
    let previous = OwnerInventory::load(&inventory_path)?.unwrap_or_default();
    let mut plan = RuntimePlan {
        home: home.to_path_buf(),
        root,
        entries: previous.clone(),
        previous,
        ..RuntimePlan::default()
    };
    let mut selected: BTreeSet<String> = BTreeSet::new();

    for relative in MANAGED_DIRS {
        let source = if let Some(mission) = relative.strip_prefix("missions/") {
            assets.join(mission)
        } else {
            package_root.join(relative)
        };
        if !source.is_dir() {
            plan.skipped.push(relative.to_string());
            continue;
        }
        let destination = home.join(relative);
        if plan_tree(&mut plan, &source, &destination, &mut selected)? {
            plan_prune(&mut plan, &destination, &selected)?;
        }
    }

    for relative in MANAGED_FILES {
        let source = package_root.join(relative);
        if !source.is_file() {
            plan.skipped.push(relative.to_string());
            continue;
        }
        let mode = observe(&source).mode.unwrap_or(0o644) & 0o7777;
        let destination = home.join(relative);
        plan_asset(&mut plan, &source, &destination, Some(mode), &mut selected)?;
    }

    // Source preserves unproven orphan staging directories beside the home.
    if let Some(parent) = home.parent()
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".kittify_update_") {
                plan.warnings.push(format!(
                    "preserved unproven orphan staging directory {}",
                    entry.path().display()
                ));
            }
        }
    }

    plan.inventory_changed = plan.entries != plan.previous;
    if read_version_lock(home).as_deref() != Some(cli_version) {
        plan.version_lock = Some(cli_version.to_string());
    }
    Ok(Some(plan))
}

/// Plan one managed tree; returns whether the destination accepted the tree.
fn plan_tree(
    plan: &mut RuntimePlan,
    source: &Path,
    destination: &Path,
    selected: &mut BTreeSet<String>,
) -> Result<bool, InstallError> {
    let source_state = observe(source);
    match source_state.kind {
        NodeKind::Directory => {}
        NodeKind::Symlink => {
            return Err(InstallError::UnsupportedNode {
                path: source.to_path_buf(),
                detail: "managed runtime assets never contain symlinks".to_string(),
            });
        }
        _ => {
            return Err(InstallError::UnsupportedNode {
                path: source.to_path_buf(),
                detail: "managed runtime asset tree is not a directory".to_string(),
            });
        }
    }
    if !plan_asset(plan, source, destination, None, selected)? {
        return Ok(false);
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(source)
        .map_err(|error| InstallError::io("read", source, error))?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        let name = child
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        let output = destination.join(&name);
        let state = observe(&child);
        match state.kind {
            NodeKind::Directory => {
                plan_tree(plan, &child, &output, selected)?;
            }
            NodeKind::File => {
                let mode = state.mode.unwrap_or(0o644) & 0o7777;
                plan_asset(plan, &child, &output, Some(mode), selected)?;
            }
            _ => {
                return Err(InstallError::UnsupportedNode {
                    path: child,
                    detail: "managed runtime assets never contain symlinks".to_string(),
                });
            }
        }
    }
    Ok(true)
}

/// Prepare one asset write using the Source ownership rule; `false` = preserved.
fn plan_asset(
    plan: &mut RuntimePlan,
    source: &Path,
    destination: &Path,
    mode: Option<u32>,
    selected: &mut BTreeSet<String>,
) -> Result<bool, InstallError> {
    let relative = relative_to_root(&plan.root, destination)?;
    selected.insert(relative.clone());
    let before = observe(destination);
    let desired = match mode {
        Some(mode) => {
            let bytes =
                std::fs::read(source).map_err(|error| InstallError::io("read", source, error))?;
            if before.kind == NodeKind::Directory {
                plan.preserved.push((
                    relative.clone(),
                    "Directory replacement requires enumerated ownership".to_string(),
                ));
                plan.effects.push(RuntimeEffect {
                    path: relative,
                    action: EffectAction::Preserve,
                    before: before.clone(),
                    after: before,
                    reason: "a directory is never replaced by a managed file".to_string(),
                });
                return Ok(false);
            }
            NodeState::file(crate::manifest::content_hash(&bytes), mode)
        }
        None => {
            if before.kind == NodeKind::File {
                plan.preserved.push((
                    relative.clone(),
                    "Unproven asset tree replacement".to_string(),
                ));
                plan.effects.push(RuntimeEffect {
                    path: relative,
                    action: EffectAction::Preserve,
                    before: before.clone(),
                    after: before,
                    reason: "a managed tree is never written through a regular file".to_string(),
                });
                return Ok(false);
            }
            NodeState::directory(MANAGED_DIR_MODE)
        }
    };
    let old = plan.previous.entries.get(&relative);
    let equal = before.content_matches(&desired);
    // Source `asset(..., managed_tree=True)`: a symlink destination is the one
    // unproven node kind that is never replaced.
    if before.kind == NodeKind::Symlink {
        plan.preserved.push((
            relative.clone(),
            if old.is_some() {
                "Changed managed asset".to_string()
            } else {
                "Unproven existing asset".to_string()
            },
        ));
        plan.effects.push(RuntimeEffect {
            path: relative,
            action: EffectAction::Preserve,
            before,
            after: NodeState::absent(),
            reason: "a symlink at a managed destination is never replaced".to_string(),
        });
        return Ok(false);
    }
    let action = match (before.kind, equal) {
        (NodeKind::Absent, _) => EffectAction::Create,
        (_, true) if before.mode != desired.mode => EffectAction::Chmod,
        (_, true) => {
            // Already matching: keep the previous record byte-identical.
            return Ok(true);
        }
        _ => EffectAction::Update,
    };
    plan.sources.insert(relative.clone(), source.to_path_buf());
    plan.effects.push(RuntimeEffect {
        path: relative.clone(),
        action,
        before,
        after: desired.clone(),
        reason: "refresh canonical global asset".to_string(),
    });
    plan.entries.entries.insert(relative, desired);
    Ok(true)
}

/// Retire recorded descendants of one managed destination that the package no
/// longer ships, preserving any node whose state no longer matches its record.
fn plan_prune(
    plan: &mut RuntimePlan,
    destination: &Path,
    selected: &BTreeSet<String>,
) -> Result<(), InstallError> {
    let prefix = format!("{}/", relative_to_root(&plan.root, destination)?);
    let mut candidates: Vec<String> = plan
        .previous
        .entries
        .keys()
        .filter(|relative| relative.starts_with(&prefix))
        .cloned()
        .collect();
    candidates.sort_by_key(|relative| std::cmp::Reverse(relative.matches('/').count()));
    for relative in candidates {
        if selected.contains(&relative) {
            continue;
        }
        let path = plan.root.join(&relative);
        let before = observe(&path);
        if before.kind == NodeKind::Absent {
            // Source `retire` keeps the record: an absent recorded node is not a
            // proven deletion, so the entry survives.
            continue;
        }
        let old = plan
            .previous
            .entries
            .get(&relative)
            .cloned()
            .unwrap_or_default();
        if !before.ownership_matches(&old) {
            plan.preserved.push((
                relative.clone(),
                "Unproven or edited retired asset".to_string(),
            ));
            plan.effects.push(RuntimeEffect {
                path: relative,
                action: EffectAction::Preserve,
                before: before.clone(),
                after: before,
                reason: "retired asset no longer matches its recorded state".to_string(),
            });
            continue;
        }
        if before.kind == NodeKind::Directory {
            let holds_recorded = plan
                .entries
                .entries
                .keys()
                .any(|child| child.starts_with(&format!("{relative}/")));
            // Descendants already planned for deletion are still present on
            // disk. Any other direct child is preserved content, including an
            // unrecorded operator file (never infer ownership from its name).
            let children = std::fs::read_dir(&path)
                .map_err(|error| InstallError::io("read retired directory", &path, error))?;
            let mut holds_unrecorded = false;
            for child in children {
                let child = child
                    .map_err(|error| InstallError::io("read retired directory", &path, error))?
                    .path();
                if !plan.effects.iter().any(|effect| {
                    effect.action == EffectAction::Delete && plan.root.join(&effect.path) == child
                }) {
                    holds_unrecorded = true;
                    break;
                }
            }
            if holds_recorded || holds_unrecorded {
                plan.preserved.push((
                    relative.clone(),
                    "Retired directory contains preserved content".to_string(),
                ));
                continue;
            }
        }
        plan.effects.push(RuntimeEffect {
            path: relative.clone(),
            action: EffectAction::Delete,
            before,
            after: NodeState::absent(),
            reason: "recorded asset the current package no longer ships".to_string(),
        });
        plan.entries.entries.remove(&relative);
    }
    Ok(())
}

/// Provision the user-global runtime home from the package assets.
///
/// `Ok(None)` when this build has no package assets to provision from (a bare
/// binary outside a checkout/install): the caller must report that honestly
/// rather than claim a runtime it could not write.
pub fn ensure_runtime(
    home: &Path,
    cli_version: &str,
) -> Result<Option<RuntimeProvisionReport>, InstallError> {
    let user_home = sk_kernel::paths::home_directory().unwrap_or_else(|_| home.to_path_buf());
    ensure_runtime_with(home, &user_home, cli_version)
}

/// [`ensure_runtime`] with an explicit user home (see [`plan_runtime_with`]).
pub fn ensure_runtime_with(
    home: &Path,
    user_home: &Path,
    cli_version: &str,
) -> Result<Option<RuntimeProvisionReport>, InstallError> {
    if package_missions_root()
        .filter(|assets| assets.is_dir())
        .is_none()
    {
        return Ok(None);
    }
    // Source does not acquire (and therefore does not refresh) an owner lock
    // for an already-converged pass. Recheck effectful plans under the lock.
    let Some(initial_plan) = plan_runtime_with(home, user_home, cli_version)? else {
        return Ok(None);
    };
    if !initial_plan.changed() {
        return Ok(Some(provision_report(&initial_plan, None)));
    }
    let cache = home.join("cache");
    std::fs::create_dir_all(&cache)
        .map_err(|error| InstallError::io("create runtime cache", &cache, error))?;
    set_mode(&cache, MANAGED_DIR_MODE)?;
    let lock_path = cache.join(UPDATE_LOCK_FILE);
    let lock = sk_kernel::FileLock::acquire(&lock_path, DEFAULT_LOCK_WAIT).map_err(|error| {
        InstallError::LockUnavailable {
            path: lock_path.clone(),
            waited_ms: u64::try_from(DEFAULT_LOCK_WAIT.as_millis()).unwrap_or(u64::MAX),
            source: std::io::Error::other(error.to_string()),
        }
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        lock.set_permissions(std::fs::Permissions::from_mode(0o644))
            .map_err(|error| {
                InstallError::io(
                    "set lock mode",
                    &lock_path,
                    std::io::Error::other(error.to_string()),
                )
            })?;
    }
    // Plan under the held lock: the observations applied are the ones planned.
    let Some(plan) = plan_runtime_with(home, user_home, cli_version)? else {
        return Ok(None);
    };
    if plan.changed() && observe(&lock_path).kind != NodeKind::File {
        return Err(InstallError::UpgradeRefused {
            detail: "Global owner lock is not a regular file".to_owned(),
        });
    }
    let mut report = provision_report(&plan, Some(&lock_path));
    for effect in &plan.effects {
        let path = plan.root.join(&effect.path);
        match effect.action {
            EffectAction::Delete => {
                let result = if effect.before.kind == NodeKind::Directory {
                    std::fs::remove_dir(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                result.map_err(|error| InstallError::io("remove", &path, error))?;
                report.pruned.push(effect.path.clone());
            }
            EffectAction::Create | EffectAction::Update | EffectAction::Chmod => {
                if effect.after.kind == NodeKind::Directory {
                    std::fs::create_dir_all(&path)
                        .map_err(|error| InstallError::io("create directory", &path, error))?;
                    report.directories.push(effect.path.clone());
                } else {
                    let Some(source) = plan.sources.get(&effect.path) else {
                        return Err(InstallError::PreconditionChanged { path: path.clone() });
                    };
                    let bytes = std::fs::read(source)
                        .map_err(|error| InstallError::io("read", source, error))?;
                    if effect.action != EffectAction::Chmod {
                        write_with_mode(&path, &bytes, effect.after.mode.unwrap_or(0o644))?;
                    }
                    report.files.push(effect.path.clone());
                }
                if let Some(mode) = effect.after.mode {
                    set_mode(&path, mode)?;
                }
            }
            EffectAction::Preserve => {}
        }
    }
    if plan.inventory_changed {
        let inventory_path = cache.join(INVENTORY_FILE);
        write_with_mode(&inventory_path, plan.entries.render().as_bytes(), 0o644)?;
        report.inventory = Some(inventory_path.display().to_string());
    }
    if let Some(version) = &plan.version_lock {
        let version_path = cache.join(VERSION_LOCK_FILE);
        write_with_mode(&version_path, version.as_bytes(), 0o644)?;
        report.version_lock = Some(version.clone());
    }
    // The canonical guard clears ONLY its held descriptor on release. Never
    // reopen the name: a peer may have replaced it with a foreign inode/link.
    // An effectful release has its real mtime, not a restored stability stamp.
    drop(lock);
    Ok(Some(report))
}

fn provision_report(plan: &RuntimePlan, lock_path: Option<&Path>) -> RuntimeProvisionReport {
    RuntimeProvisionReport {
        home: plan.home.clone(),
        lock: lock_path.map(|path| path.display().to_string()),
        preserved: plan
            .preserved
            .iter()
            .map(|(path, reason)| format!("{path}: {reason}"))
            .collect(),
        skipped: plan.skipped.clone(),
        warnings: plan.warnings.clone(),
        ..RuntimeProvisionReport::default()
    }
}

fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<(), InstallError> {
    #[cfg(unix)]
    let result = {
        use std::os::unix::fs::PermissionsExt;
        sk_kernel::atomic_write_with_permissions(path, bytes, std::fs::Permissions::from_mode(mode))
    };
    #[cfg(not(unix))]
    let result = {
        let _ = mode;
        sk_kernel::atomic_write(path, bytes)
    };
    result
        .map(|_| ())
        .map_err(|error| InstallError::io("write", path, std::io::Error::other(error.to_string())))
}

fn set_mode(path: &Path, mode: u32) -> Result<(), InstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|error| InstallError::io("set mode", path, error))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}
