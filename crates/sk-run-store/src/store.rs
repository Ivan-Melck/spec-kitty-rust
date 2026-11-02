//! Durable run layout, the locked `feature-runs.json` index and run start.
//!
//! Layout (exactly upstream):
//!
//! ```text
//! <project>/.kittify/runtime/feature-runs.json              # identity index
//! <project>/.kittify/runtime/feature-runs.json.lock         # index-only sidecar
//! <project>/.kittify/runtime/runs/<32-hex>/state.json       # cursor
//! <project>/.kittify/runtime/runs/<32-hex>/mission_template_frozen.yaml
//! <project>/.kittify/runtime/runs/<32-hex>/run.events.jsonl
//! <project>/.kittify/runtime/runs/<32-hex>/state.json.lock  # cursor-only sidecar
//! <project>/.kittify/runtime/runs/<32-hex>/prompts/<step>.prompt.md
//! ```

use std::collections::{BTreeMap, hash_map::RandomState};
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sk_kernel::{FileLock, KernelError};
use sk_runtime::MissionTemplate;

use crate::error::{RunStoreError, timeout_error};
use crate::events::{EventRecord, MISSION_RUN_STARTED, append_records};
use crate::model::{
    DEFAULT_LOCK_TIMEOUT, MAX_INDEX_BYTES, MAX_STATE_BYTES, RunRef, RunState, StartOutcome,
    StartRequest, now_iso,
};
use crate::prompt::prompt_file_path;

pub const STATE_FILENAME: &str = "state.json";
pub const FROZEN_TEMPLATE_FILENAME: &str = "mission_template_frozen.yaml";
pub const EVENTS_FILENAME: &str = "run.events.jsonl";
pub const INDEX_FILENAME: &str = "feature-runs.json";
pub const LOCK_SUFFIX: &str = ".lock";
pub const LEGACY_KEY_PREFIX: &str = "legacy-";
pub const RUN_ID_HEX_LENGTH: usize = 32;

static RUN_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The governed run store for one project root.
#[derive(Debug, Clone)]
pub struct RunStore {
    root: PathBuf,
    lock_timeout: Duration,
}

impl RunStore {
    /// Open the store for an existing project root.
    ///
    /// This performs no writes: a project without `.kittify/runtime` is valid
    /// and simply has no runs yet.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, RunStoreError> {
        let root = fs::canonicalize(root.as_ref()).map_err(|source| {
            RunStoreError::Kernel(KernelError::Io {
                action: "resolve project root",
                path: root.as_ref().to_path_buf(),
                source,
            })
        })?;
        if !root.is_dir() {
            return Err(RunStoreError::RootNotDirectory { root });
        }
        Ok(Self {
            root,
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        })
    }

    /// Override the bounded wait for the cursor and index locks.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join(".kittify").join("runtime")
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.runtime_dir().join("runs")
    }

    pub fn index_path(&self) -> PathBuf {
        self.runtime_dir().join(INDEX_FILENAME)
    }

    pub fn index_lock_path(&self) -> PathBuf {
        self.runtime_dir()
            .join(format!("{INDEX_FILENAME}{LOCK_SUFFIX}"))
    }

    pub fn state_path(&self, run: &RunRef) -> PathBuf {
        run.run_dir.join(STATE_FILENAME)
    }

    pub fn frozen_template_path(&self, run: &RunRef) -> PathBuf {
        run.run_dir.join(FROZEN_TEMPLATE_FILENAME)
    }

    pub fn events_path(&self, run: &RunRef) -> PathBuf {
        run.run_dir.join(EVENTS_FILENAME)
    }

    pub fn cursor_lock_path(&self, run: &RunRef) -> PathBuf {
        run.run_dir.join(format!("{STATE_FILENAME}{LOCK_SUFFIX}"))
    }

    /// Acquire the one shared per-run cursor lock.
    pub(crate) fn lock_cursor(&self, run: &RunRef) -> Result<FileLock, RunStoreError> {
        let path = self.cursor_lock_path(run);
        FileLock::acquire(&path, self.lock_timeout).map_err(|error| map_lock_error(path, error))
    }

    pub(crate) fn lock_index(&self) -> Result<FileLock, RunStoreError> {
        let path = self.index_lock_path();
        FileLock::acquire(&path, self.lock_timeout).map_err(|error| map_lock_error(path, error))
    }

    /// Start a governed run, or resume the mission's indexed live run.
    pub fn start(&self, request: &StartRequest) -> Result<StartOutcome, RunStoreError> {
        validate_mission_slug(&request.mission_slug)?;
        if let Some(mission_id) = &request.mission_id {
            validate_mission_id(mission_id)?;
        }

        // Phase 1: reuse an indexed live run under the index lock.
        let mut orphans_removed = 0usize;
        {
            let _lock = self.lock_index()?;
            let (index, rekeyed) = self.read_index_canonical()?;
            if let Some((_key, entry)) =
                find_entry(&index, &request.mission_slug, request.mission_id.as_deref())?
            {
                let run = self.run_ref_from_entry(entry)?;
                if rekeyed {
                    self.write_index(&index)?;
                }
                let state = self.read_state(&run)?;
                return Ok(StartOutcome {
                    run,
                    state,
                    started: false,
                    resumed: true,
                    orphans_removed,
                });
            }
        }

        // Phase 2: create the run outside the index lock (slow template I/O and
        // a unique run id must not hold the index).
        let created = self.create_run(request)?;

        // Phase 3: commit the index under the lock, re-checking that no peer won.
        let _lock = self.lock_index()?;
        let created_dir = created.0.run_dir.clone();
        let committed = (|| {
            let (mut index, _rekeyed) = self.read_index_canonical()?;
            if let Some((_key, entry)) =
                find_entry(&index, &request.mission_slug, request.mission_id.as_deref())?
            {
                let winner = self.run_ref_from_entry(entry)?;
                if winner.run_id != created.0.run_id
                    && created.0.run_dir.starts_with(self.runs_dir())
                {
                    // We lost the same-mission race: remove only our own new run.
                    let _ = fs::remove_dir_all(&created.0.run_dir);
                    orphans_removed += 1;
                }
                let state = self.read_state(&winner)?;
                return Ok(StartOutcome {
                    run: winner,
                    state,
                    started: false,
                    resumed: true,
                    orphans_removed,
                });
            }

            let (run, state) = created;
            let key = run_index_key(&request.mission_slug, request.mission_id.as_deref());
            index.insert(
                key,
                json!({
                    "run_id": run.run_id,
                    "run_dir": self.run_dir_token(&run.run_dir)?,
                    "mission_type": state.mission_key,
                    "mission_key": state.mission_key,
                    "mission_id": state.mission_id,
                    "mission_slug": state.mission_slug,
                }),
            );
            self.write_index(&index)?;
            Ok(StartOutcome {
                run,
                state,
                started: true,
                resumed: false,
                orphans_removed,
            })
        })();
        if committed.is_err() {
            // No index entry names this run (the lock is still held and the
            // write failed or never happened), so it is unreachable.
            let _ = fs::remove_dir_all(&created_dir);
        }
        committed
    }

    fn create_run(&self, request: &StartRequest) -> Result<(RunRef, RunState), RunStoreError> {
        fs::create_dir_all(self.runs_dir()).map_err(|source| {
            RunStoreError::Kernel(KernelError::Io {
                action: "create run store",
                path: self.runs_dir(),
                source,
            })
        })?;

        let source = fs::canonicalize(&request.template_path).map_err(|source| {
            RunStoreError::Kernel(KernelError::Io {
                action: "resolve mission template",
                path: request.template_path.clone(),
                source,
            })
        })?;
        let metadata = fs::metadata(&source).map_err(|error| {
            RunStoreError::Kernel(KernelError::Io {
                action: "inspect mission template",
                path: source.clone(),
                source: error,
            })
        })?;
        if !metadata.is_file() {
            return Err(RunStoreError::NotAFile { path: source });
        }
        let template_bytes = sk_kernel::read_bounded(&source, sk_runtime::MAX_TEMPLATE_BYTES)?;
        let template_hash = sha256_hex(&template_bytes);

        let mut run_dir = None;
        for _ in 0..16 {
            let run_id = next_run_id();
            let candidate = self.runs_dir().join(&run_id);
            match fs::create_dir(&candidate) {
                Ok(()) => {
                    run_dir = Some((run_id, candidate));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(RunStoreError::Kernel(KernelError::Io {
                        action: "create run directory",
                        path: candidate,
                        source,
                    }));
                }
            }
        }
        let Some((run_id, run_dir)) = run_dir else {
            return Err(RunStoreError::IndexCorrupt {
                path: self.runs_dir(),
                message: "could not allocate a unique run id after 16 attempts".to_string(),
            });
        };

        let result = (|| {
            sk_kernel::atomic_write(&run_dir.join(FROZEN_TEMPLATE_FILENAME), &template_bytes)?;
            let frozen_path = run_dir.join(FROZEN_TEMPLATE_FILENAME);
            let template: MissionTemplate = sk_runtime::load_template(&frozen_path)?;
            let frozen_hash = sha256_hex(&sk_kernel::read_bounded(
                &frozen_path,
                sk_runtime::MAX_TEMPLATE_BYTES,
            )?);
            if frozen_hash != template_hash {
                return Err(RunStoreError::FrozenTemplateDrift {
                    run_id: run_id.clone(),
                    expected: template_hash,
                    found: frozen_hash,
                });
            }

            let mut inputs = request.inputs.clone();
            inputs
                .entry("mission_slug".to_string())
                .or_insert_with(|| Value::String(request.mission_slug.clone()));

            let state = RunState {
                run_id: run_id.clone(),
                mission_key: template.mission.key.clone(),
                template_path: source.to_string_lossy().into_owned(),
                template_hash: template_hash.clone(),
                policy_snapshot: request.policy_snapshot.clone(),
                completed_steps: Vec::new(),
                issued_step_id: None,
                inputs,
                decisions: BTreeMap::new(),
                pending_decisions: BTreeMap::new(),
                blocked_reason: None,
                mission_id: request.mission_id.clone(),
                mission_slug: Some(request.mission_slug.clone()),
            };
            write_state_file(&run_dir, &state)?;

            let started = EventRecord::new(
                MISSION_RUN_STARTED,
                json!({
                    "run_id": run_id,
                    "mission_type": template.mission.key,
                    "actor": {
                        "actor_id": "system",
                        "actor_type": "service",
                        "display_name": "",
                        "provider": null,
                        "model": null,
                        "tool": null,
                    },
                    "mission_id": request.mission_id,
                    "mission_slug": request.mission_slug,
                }),
            );
            append_records(&run_dir.join(EVENTS_FILENAME), &[started])?;

            let run = RunRef {
                run_id,
                run_dir: run_dir.clone(),
                mission_key: template.mission.key,
                mission_id: request.mission_id.clone(),
                mission_slug: Some(request.mission_slug.clone()),
            };
            Ok((run, state))
        })();

        if result.is_err() {
            // A failed start must not leave a half-created run the index could
            // later point at. Only the directory this call created is removed.
            let _ = fs::remove_dir_all(&run_dir);
        }
        result
    }

    /// Validate and canonicalize a caller-supplied run reference.
    ///
    /// The reference must name a 32-hex run id inside this project's run store
    /// whose directory name and durable cursor agree. A forged or foreign path
    /// is refused before any lock is taken.
    pub(crate) fn resolve_ref(&self, run: &RunRef) -> Result<RunRef, RunStoreError> {
        validate_run_id(&run.run_id)?;
        let relative =
            run.run_dir
                .strip_prefix(&self.root)
                .map_err(|_| RunStoreError::RunDirOutsideRepo {
                    token: run.run_dir.to_string_lossy().into_owned(),
                    resolved: run.run_dir.clone(),
                    root: self.root.clone(),
                })?;
        let resolved =
            sk_kernel::resolve_within(&self.root, relative).map_err(|error| match error {
                KernelError::PathEscape { root, resolved } => RunStoreError::RunDirOutsideRepo {
                    token: run.run_dir.to_string_lossy().into_owned(),
                    resolved,
                    root,
                },
                other => RunStoreError::Kernel(other),
            })?;
        let file_name = resolved.file_name().and_then(|name| name.to_str());
        if file_name != Some(run.run_id.as_str()) {
            return Err(RunStoreError::IdentityMismatch {
                message: format!(
                    "run reference {:?} does not name directory {:?}",
                    run.run_id, file_name
                ),
            });
        }
        let state = self.read_state_at(&resolved, Some(&run.run_id))?;
        Ok(RunRef {
            run_id: run.run_id.clone(),
            run_dir: resolved,
            mission_key: state.mission_key,
            mission_id: state.mission_id,
            mission_slug: state.mission_slug,
        })
    }

    /// Read the run cursor, validating its baseline invariants.
    pub fn read_state(&self, run: &RunRef) -> Result<RunState, RunStoreError> {
        self.read_state_at(&run.run_dir, Some(&run.run_id))
    }

    pub(crate) fn read_state_at(
        &self,
        run_dir: &Path,
        expected_run_id: Option<&str>,
    ) -> Result<RunState, RunStoreError> {
        let path = run_dir.join(STATE_FILENAME);
        let bytes =
            sk_kernel::read_bounded(&path, MAX_STATE_BYTES).map_err(|error| match error {
                KernelError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                    RunStoreError::RunStateMissing {
                        run_id: expected_run_id.unwrap_or("<unknown>").to_string(),
                        run_dir: run_dir.to_path_buf(),
                    }
                }
                other => RunStoreError::Kernel(other),
            })?;
        let state: RunState =
            serde_json::from_slice(&bytes).map_err(|error| RunStoreError::StateCorrupt {
                path: path.clone(),
                message: error.to_string(),
            })?;
        state.validate(expected_run_id)?;
        Ok(state)
    }

    /// Resolve the mission's indexed run without starting one or writing the
    /// canonical (rekeyed) view back.
    pub fn run_for_mission(
        &self,
        mission_slug: &str,
        mission_id: Option<&str>,
    ) -> Result<Option<RunRef>, RunStoreError> {
        validate_mission_slug(mission_slug)?;
        if let Some(mission_id) = mission_id {
            validate_mission_id(mission_id)?;
        }
        let (index, _rekeyed) = self.read_index_canonical()?;
        let Some((_key, entry)) = find_entry(&index, mission_slug, mission_id)? else {
            return Ok(None);
        };
        self.run_ref_from_entry(entry).map(Some)
    }

    pub(crate) fn read_index_canonical(
        &self,
    ) -> Result<(BTreeMap<String, Value>, bool), RunStoreError> {
        let index = self.read_index_raw()?;
        Ok(canonicalize_index(index))
    }

    fn read_index_raw(&self) -> Result<BTreeMap<String, Value>, RunStoreError> {
        let path = self.index_path();
        let bytes = match sk_kernel::read_bounded(&path, MAX_INDEX_BYTES) {
            Ok(bytes) => bytes,
            Err(KernelError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(BTreeMap::new());
            }
            Err(error) => return Err(error.into()),
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(BTreeMap::new());
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|error| RunStoreError::IndexCorrupt {
                path: path.clone(),
                message: error.to_string(),
            })?;
        let object = value
            .as_object()
            .ok_or_else(|| RunStoreError::IndexCorrupt {
                path: path.clone(),
                message: "top-level value must be an object".to_string(),
            })?;
        let mut index = BTreeMap::new();
        for (key, entry) in object {
            let entry_object = entry
                .as_object()
                .ok_or_else(|| RunStoreError::IndexCorrupt {
                    path: path.clone(),
                    message: format!("entry {key:?} must be an object"),
                })?;
            for field in ["run_id", "run_dir"] {
                if !entry_object.get(field).is_some_and(Value::is_string) {
                    return Err(RunStoreError::IndexCorrupt {
                        path: path.clone(),
                        message: format!("entry {key:?} is missing string field {field:?}"),
                    });
                }
            }
            index.insert(key.clone(), entry.clone());
        }
        Ok(index)
    }

    pub(crate) fn write_index(&self, index: &BTreeMap<String, Value>) -> Result<(), RunStoreError> {
        fs::create_dir_all(self.runtime_dir()).map_err(|source| {
            RunStoreError::Kernel(KernelError::Io {
                action: "create runtime directory",
                path: self.runtime_dir(),
                source,
            })
        })?;
        let mut object = Map::new();
        for (key, value) in index {
            object.insert(key.clone(), value.clone());
        }
        let bytes = serde_json::to_vec_pretty(&Value::Object(object)).map_err(|error| {
            RunStoreError::IndexCorrupt {
                path: self.index_path(),
                message: format!("could not serialize index: {error}"),
            }
        })?;
        sk_kernel::atomic_write(&self.index_path(), &bytes)?;
        Ok(())
    }

    pub(crate) fn ensure_index_entry(
        &self,
        run: &RunRef,
        state: &RunState,
    ) -> Result<bool, RunStoreError> {
        let _lock = self.lock_index()?;
        let (mut index, _rekeyed) = self.read_index_canonical()?;
        if let Some((_key, entry)) = find_entry(
            &index,
            state
                .mission_slug
                .as_deref()
                .ok_or_else(|| RunStoreError::IdentityMismatch {
                    message: format!("run {:?} has no mission_slug to index", run.run_id),
                })?,
            state.mission_id.as_deref(),
        )? {
            let indexed = self.run_ref_from_entry(entry)?;
            if indexed.run_id == run.run_id {
                return Ok(false);
            }
            return Err(RunStoreError::IdentityMismatch {
                message: format!(
                    "mission {:?} is indexed to run {:?}, not {:?}; repair the index explicitly",
                    state.mission_slug, indexed.run_id, run.run_id
                ),
            });
        }
        let key = run_index_key(
            state.mission_slug.as_deref().unwrap_or_default(),
            state.mission_id.as_deref(),
        );
        index.insert(
            key,
            json!({
                "run_id": run.run_id,
                "run_dir": self.run_dir_token(&run.run_dir)?,
                "mission_type": state.mission_key,
                "mission_key": state.mission_key,
                "mission_id": state.mission_id,
                "mission_slug": state.mission_slug,
            }),
        );
        self.write_index(&index)?;
        Ok(true)
    }

    pub(crate) fn run_ref_from_entry(&self, entry: &Value) -> Result<RunRef, RunStoreError> {
        let object = entry
            .as_object()
            .ok_or_else(|| RunStoreError::IndexCorrupt {
                path: self.index_path(),
                message: "entry is not an object".to_string(),
            })?;
        let string = |field: &str| {
            object
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let run_id = string("run_id").ok_or_else(|| RunStoreError::IndexCorrupt {
            path: self.index_path(),
            message: "entry is missing run_id".to_string(),
        })?;
        validate_run_id(&run_id)?;
        let token = string("run_dir").ok_or_else(|| RunStoreError::IndexCorrupt {
            path: self.index_path(),
            message: format!("entry {run_id:?} is missing run_dir"),
        })?;
        let relative = Path::new(&token);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::Prefix(_) | Component::RootDir | Component::ParentDir
                )
            })
        {
            return Err(RunStoreError::RunDirOutsideRepo {
                token: token.clone(),
                resolved: self.root.join(relative),
                root: self.root.clone(),
            });
        }
        let resolved =
            sk_kernel::resolve_within(&self.root, relative).map_err(|error| match error {
                KernelError::PathEscape { root, resolved } => RunStoreError::RunDirOutsideRepo {
                    token,
                    resolved,
                    root,
                },
                other => RunStoreError::Kernel(other),
            })?;
        let file_name = resolved.file_name().and_then(|name| name.to_str());
        if file_name != Some(run_id.as_str()) {
            return Err(RunStoreError::IdentityMismatch {
                message: format!(
                    "index run_id {run_id:?} does not match run directory name {file_name:?}"
                ),
            });
        }
        let state = self.read_state_at(&resolved, Some(&run_id))?;
        for field in ["mission_id", "mission_slug"] {
            let indexed = string(field);
            let durable = match field {
                "mission_id" => state.mission_id.clone(),
                _ => state.mission_slug.clone(),
            };
            if let (Some(indexed), Some(durable)) = (indexed, durable)
                && indexed != durable
            {
                return Err(RunStoreError::IdentityMismatch {
                    message: format!(
                        "index {field} {indexed:?} does not match cursor {durable:?} for run {run_id:?}"
                    ),
                });
            }
        }
        Ok(RunRef {
            run_id,
            run_dir: resolved,
            mission_key: string("mission_key")
                .or_else(|| string("mission_type"))
                .unwrap_or_else(|| state.mission_key.clone()),
            mission_id: string("mission_id").or(state.mission_id),
            mission_slug: string("mission_slug").or(state.mission_slug),
        })
    }

    pub(crate) fn run_dir_token(&self, run_dir: &Path) -> Result<String, RunStoreError> {
        let relative =
            run_dir
                .strip_prefix(&self.root)
                .map_err(|_| RunStoreError::RunDirOutsideRepo {
                    token: run_dir.to_string_lossy().into_owned(),
                    resolved: run_dir.to_path_buf(),
                    root: self.root.clone(),
                })?;
        let mut parts = Vec::new();
        for component in relative.components() {
            match component {
                Component::Normal(part) => parts.push(
                    part.to_str()
                        .ok_or_else(|| RunStoreError::IdentityMismatch {
                            message: format!(
                                "run directory {run_dir:?} has a non-UTF-8 component and cannot be indexed"
                            ),
                        })?
                        .to_string(),
                ),
                _ => {
                    return Err(RunStoreError::RunDirOutsideRepo {
                        token: run_dir.to_string_lossy().into_owned(),
                        resolved: run_dir.to_path_buf(),
                        root: self.root.clone(),
                    });
                }
            }
        }
        Ok(parts.join("/"))
    }

    pub(crate) fn frozen_template_bytes(&self, run: &RunRef) -> Result<Vec<u8>, RunStoreError> {
        let path = self.frozen_template_path(run);
        let bytes =
            sk_kernel::read_bounded(&path, sk_runtime::MAX_TEMPLATE_BYTES).map_err(|error| {
                match error {
                    KernelError::Io { source, .. }
                        if source.kind() == std::io::ErrorKind::NotFound =>
                    {
                        RunStoreError::StateCorrupt {
                            path: path.clone(),
                            message: "frozen mission template is missing".to_string(),
                        }
                    }
                    other => RunStoreError::Kernel(other),
                }
            })?;
        Ok(bytes)
    }

    pub(crate) fn verify_frozen_hash(
        &self,
        run: &RunRef,
        state: &RunState,
    ) -> Result<Vec<u8>, RunStoreError> {
        let bytes = self.frozen_template_bytes(run)?;
        let found = sha256_hex(&bytes);
        if found != state.template_hash {
            return Err(RunStoreError::FrozenTemplateDrift {
                run_id: run.run_id.clone(),
                expected: state.template_hash.clone(),
                found,
            });
        }
        Ok(bytes)
    }

    pub(crate) fn load_frozen_template(
        &self,
        run: &RunRef,
    ) -> Result<MissionTemplate, RunStoreError> {
        let path = self.frozen_template_path(run);
        Ok(sk_runtime::load_template(&path)?)
    }

    /// The recorded live template hash, or `None` when the recorded source no
    /// longer exists (a missing live file is not drift, matching upstream).
    pub(crate) fn live_template_hash(
        &self,
        state: &RunState,
    ) -> Result<Option<String>, RunStoreError> {
        if state.template_path.is_empty() {
            return Ok(None);
        }
        let path = Path::new(&state.template_path);
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => {
                let bytes = sk_kernel::read_bounded(path, sk_runtime::MAX_TEMPLATE_BYTES).map_err(
                    |error| RunStoreError::LiveTemplateUnreadable {
                        path: path.to_path_buf(),
                        message: error.to_string(),
                    },
                )?;
                Ok(Some(sha256_hex(&bytes)))
            }
            Ok(_) => Err(RunStoreError::LiveTemplateUnreadable {
                path: path.to_path_buf(),
                message: "recorded live template is not a regular file".to_string(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(RunStoreError::LiveTemplateUnreadable {
                path: path.to_path_buf(),
                message: error.to_string(),
            }),
        }
    }

    /// The real, contained prompt artifact path for a step.
    pub(crate) fn prompt_path(
        &self,
        run: &RunRef,
        step_id: &str,
    ) -> Result<PathBuf, RunStoreError> {
        prompt_file_path(&run.run_dir, step_id)
    }
}

/// Validate the canonical mission slug grammar used by this crate.
pub fn validate_mission_slug(slug: &str) -> Result<(), RunStoreError> {
    let safe = !slug.is_empty()
        && slug.len() <= 200
        && slug.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        && !slug.contains("..")
        && slug != "."
        && !slug.starts_with('.');
    if safe {
        Ok(())
    } else {
        Err(RunStoreError::InvalidMissionSlug {
            value: slug.to_string(),
        })
    }
}

/// Validate a mission identity token (ULID or `legacy-` compatible key).
pub fn validate_mission_id(mission_id: &str) -> Result<(), RunStoreError> {
    let safe = !mission_id.is_empty()
        && mission_id.len() <= 200
        && mission_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        && !mission_id.contains("..")
        && mission_id != "."
        && !mission_id.starts_with('.');
    if safe {
        Ok(())
    } else {
        Err(RunStoreError::InvalidMissionId {
            value: mission_id.to_string(),
        })
    }
}

/// Validate the canonical `uuid4().hex` run id shape.
pub fn validate_run_id(run_id: &str) -> Result<(), RunStoreError> {
    if run_id.len() == RUN_ID_HEX_LENGTH
        && run_id
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(RunStoreError::InvalidRunId {
            value: run_id.to_string(),
        })
    }
}

/// The canonical index key: `mission_id`, else `legacy-<slug>`.
pub fn run_index_key(mission_slug: &str, mission_id: Option<&str>) -> String {
    match mission_id {
        Some(mission_id) if !mission_id.is_empty() => mission_id.to_string(),
        _ => format!("{LEGACY_KEY_PREFIX}{mission_slug}"),
    }
}

fn canonicalize_index(index: BTreeMap<String, Value>) -> (BTreeMap<String, Value>, bool) {
    let original_keys: Vec<String> = index.keys().cloned().collect();
    let mut rekeyed = BTreeMap::new();
    let mut changed = false;
    for (key, mut entry) in index {
        let slug = entry
            .get("mission_slug")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| key.clone());
        let mission_id = entry.get("mission_id").and_then(Value::as_str);
        let canonical = run_index_key(&slug, mission_id);
        let target = if canonical == key
            || original_keys.iter().any(|existing| existing == &canonical)
            || rekeyed.contains_key(&canonical)
        {
            key.clone()
        } else {
            canonical
        };
        if target != key {
            if let Some(object) = entry.as_object_mut() {
                object
                    .entry("mission_slug")
                    .or_insert_with(|| Value::String(slug));
            }
            changed = true;
        }
        rekeyed.insert(target, entry);
    }
    (rekeyed, changed)
}

pub(crate) fn find_entry<'a>(
    index: &'a BTreeMap<String, Value>,
    mission_slug: &str,
    mission_id: Option<&str>,
) -> Result<Option<(&'a String, &'a Value)>, RunStoreError> {
    let key = run_index_key(mission_slug, mission_id);
    let Some((key, entry)) = index.get_key_value(&key) else {
        if let Some(mission_id) = mission_id {
            for entry in index.values() {
                let unbound = entry.get("mission_id").is_none_or(Value::is_null)
                    && entry.get("mission_slug").and_then(Value::as_str) == Some(mission_slug);
                if unbound {
                    return Err(RunStoreError::RunIdentityMigrationRequired {
                        mission_slug: mission_slug.to_string(),
                        mission_id: mission_id.to_string(),
                        run_id: entry
                            .get("run_id")
                            .and_then(Value::as_str)
                            .unwrap_or("<unknown>")
                            .to_string(),
                    });
                }
            }
        }
        return Ok(None);
    };
    Ok(Some((key, entry)))
}

fn write_state_file(run_dir: &Path, state: &RunState) -> Result<(), RunStoreError> {
    let value = serde_json::to_value(state).map_err(|error| RunStoreError::StateCorrupt {
        path: run_dir.join(STATE_FILENAME),
        message: format!("could not serialize cursor: {error}"),
    })?;
    let bytes = serde_json::to_vec_pretty(&value).map_err(|error| RunStoreError::StateCorrupt {
        path: run_dir.join(STATE_FILENAME),
        message: format!("could not serialize cursor: {error}"),
    })?;
    sk_kernel::atomic_write(&run_dir.join(STATE_FILENAME), &bytes)?;
    Ok(())
}

/// Durably write the cursor (used only by transactions holding the cursor lock).
pub(crate) fn persist_state(run_dir: &Path, state: &RunState) -> Result<(), RunStoreError> {
    write_state_file(run_dir, state)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn map_lock_error(path: PathBuf, error: KernelError) -> RunStoreError {
    match error {
        KernelError::LockTimeout { timeout, .. } => timeout_error(&path, timeout),
        other => RunStoreError::Kernel(other),
    }
}

/// `uuid4().hex`-shaped run id: 32 lowercase hex characters.
///
/// The value is an opaque durable directory name, not a secret. RandomState is
/// OS-seeded; the pid/nanosecond/counter mix removes same-instant collisions,
/// and `create_dir` is the final uniqueness authority (retried on collision).
fn next_run_id() -> String {
    let counter = RUN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut first = RandomState::new().build_hasher();
    first.write_u128(nanos);
    first.write_u32(std::process::id());
    first.write_u64(counter);
    let mut second = RandomState::new().build_hasher();
    second.write_u64(first.finish());
    second.write_u64(nanos as u64);
    second.write_u32(std::process::id());
    let high = first.finish();
    let low = second.finish();
    format!("{high:016x}{low:016x}")
}

/// The current instant used by callers that need to seed a request timestamp.
pub fn now_timestamp() -> String {
    now_iso()
}
