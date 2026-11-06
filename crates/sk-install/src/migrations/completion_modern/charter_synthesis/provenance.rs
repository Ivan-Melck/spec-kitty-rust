//! Provenance-path healing: `3.2.7_heal_provenance_paths` and
//! `4_0_0rc5_heal_template_set_provenance`.
//!
//! Companion to the emit-side fix (`charter.offering.provenance.
//! to_portable_source_path`): a project that compiled its charter or projected
//! its agent-profile manifest *before* that fix committed an operator- or
//! platform-specific absolute filesystem path for a built-in-pack source into
//! git. These two effects rewrite the carriers' healable entries to the same
//! `${SPEC_KITTY_PACKS_ROOT}/built-in/...` token the fixed emit path produces, so
//! re-running (or a fresh compile) converges to zero further changes.
//!
//! Healable scope, mirroring the excluded normalizer callers exactly:
//!
//! * catalog references whose `kind` is NOT `template_set` (the mission-template
//!   reference is a deliberately excluded normalizer caller — healing it would
//!   just be re-absoluted by the next `charter generate`), plus — for
//!   `4_0_0rc5_heal_template_set_provenance` only — precisely those
//!   `template_set` rows the pinned Source 3.2.7 migration skipped;
//! * manifest entries' `source_path` field, never `output_path`.
//!
//! Within that scope only paths the one normalizer classifies as built-in are
//! rewritten; an out-of-tree absolute source (a legitimate local-support/org
//! override) is preserved as-is.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sk_charter::provenance::{is_built_in_pack_path_with, to_portable_source_path_with};

use crate::error::InstallError;
use crate::migrations::completion_legacy::paths::write_text;
use crate::profiles::{PROFILES_MANIFEST_REL, ProfileManifest, ProfileManifestEntry};
use crate::upgrade::{MigrationOutcome, NativeMigration};
use crate::yaml_edit::{CstDocument, CstMapping, CstValue};

/// The charter bundle's compiled YAML.
pub const CHARTER_YAML: &str = ".kittify/charter/charter.yaml";
/// The top-level agent-profile manifest sidecar (its own directory, not the bundle).
pub const MANIFEST_REL: &str = PROFILES_MANIFEST_REL;

/// Catalog reference kind excluded from the generic heal: the mission-template
/// reference, a deliberately excluded normalizer caller.
const EXCLUDED_CATALOG_KIND: &str = "template_set";
const TEMPLATE_SET_KIND: &str = "template_set";
const MISSION_CONFIG_FILENAME: &str = "mission.yaml";
const BUILT_IN_PARTS: [&str; 2] = ["packs", "built-in"];
const SPEC_KITTY_PROJECT_NAME: &str = "spec-kitty-cli";
const SPEC_KITTY_REPOSITORY: &str = "github.com/spec-kitty/spec-kitty";

fn charter_yaml_path(project_root: &Path) -> PathBuf {
    project_root.join(CHARTER_YAML)
}

fn manifest_path(project_root: &Path) -> PathBuf {
    project_root.join(MANIFEST_REL)
}

fn read_document(path: &Path) -> Result<CstDocument, InstallError> {
    let text =
        std::fs::read_to_string(path).map_err(|error| InstallError::io("read", path, error))?;
    CstDocument::parse(path, &text)
}

/// The Source's `_load_charter_document`: read `charter.yaml`, failing closed as
/// `MigrationStateUnreadableError` when it cannot be parsed.
///
/// An unreadable charter is not "nothing to heal": treating it that way lets the
/// runner record a skip and stamp past this migration for good. The message keeps
/// the reference's shape (`{path} could not be read ({cause}); provenance was not
/// evaluated`) with the cause FAMILY the native decoder can prove — `OSError` for a
/// read failure, `UnicodeDecodeError` for invalid UTF-8, `YAMLError` for a parse
/// failure. The reference prints `type(exc).__name__`, i.e. ruamel's concrete
/// subclass (e.g. `ScannerError`); native has no equivalent subclass taxonomy, so it
/// names the family rather than inventing a subclass name.
fn read_unreadable_as_state(path: &Path) -> Result<CstDocument, InstallError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Err(state_unreadable(path, "OSError", Some(Box::new(error))));
        }
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            return Err(state_unreadable(
                path,
                "UnicodeDecodeError",
                Some(Box::new(error)),
            ));
        }
    };
    match CstDocument::parse(path, &text) {
        Ok(document) => Ok(document),
        Err(error) => Err(state_unreadable(path, "YAMLError", Some(Box::new(error)))),
    }
}

/// Build the typed `MigrationStateUnreadable` failure for `path`.
fn state_unreadable(
    path: &Path,
    cause: &str,
    _source: Option<Box<dyn std::error::Error + Send + Sync>>,
) -> InstallError {
    InstallError::MigrationStateUnreadable {
        path: Some(path.to_path_buf()),
        detail: format!(
            "{} could not be read ({cause}); provenance was not evaluated",
            path.display()
        ),
    }
}

fn write_document(path: &Path, document: &CstDocument) -> Result<(), InstallError> {
    if path.is_symlink() {
        return Err(InstallError::UpgradeRefused {
            detail: format!(
                "refusing to write through the symlink {}; replace or remove it first",
                path.display()
            ),
        });
    }
    write_text(path, &document.text()).map(|_| ())
}

/// One catalog reference, read from the CST so authored bytes survive.
struct CatalogReference {
    id: Option<String>,
    kind: Option<String>,
    source_path: Option<String>,
}

fn catalog_references(document: &CstDocument) -> Vec<CatalogReference> {
    let Some(catalog) = document.root().get_mapping("catalog") else {
        return Vec::new();
    };
    let Some(references) = catalog.get_sequence("references") else {
        return Vec::new();
    };
    references
        .values()
        .into_iter()
        .filter_map(|value| match value {
            CstValue::Mapping(mapping, _) => Some(CatalogReference {
                id: mapping.get_string("id"),
                kind: mapping.get_string("kind"),
                source_path: mapping.get_string("source_path"),
            }),
            _ => None,
        })
        .collect()
}

/// The catalog sequence members as editable mappings, in document order.
fn catalog_reference_mappings(document: &CstDocument) -> Vec<CstMapping> {
    let Some(catalog) = document.root().get_mapping("catalog") else {
        return Vec::new();
    };
    let Some(references) = catalog.get_sequence("references") else {
        return Vec::new();
    };
    references
        .values()
        .into_iter()
        .filter_map(|value| match value {
            CstValue::Mapping(mapping, _) => Some(mapping),
            _ => None,
        })
        .collect()
}

fn is_absolute(raw: &str) -> bool {
    Path::new(raw).is_absolute()
}

fn catalog_ref_is_healable(reference: &CatalogReference, built_in: Option<&Path>) -> bool {
    if reference.kind.as_deref() == Some(EXCLUDED_CATALOG_KIND) {
        return false;
    }
    let Some(source_path) = reference.source_path.as_deref() else {
        return false;
    };
    if source_path.is_empty() || !is_absolute(source_path) {
        return false;
    }
    is_built_in_pack_path_with(Path::new(source_path), built_in)
}

/// The healable catalog change lines, plus the entries to rewrite.
fn healable_catalog(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Vec<String>, InstallError> {
    let path = charter_yaml_path(project_root);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let document = read_document(&path)?;
    Ok(catalog_references(&document)
        .iter()
        .filter(|reference| catalog_ref_is_healable(reference, built_in))
        .map(|reference| {
            format!(
                "charter.yaml catalog[{}].source_path: {} -> {}",
                reference.id.as_deref().unwrap_or("?"),
                reference.source_path.clone().unwrap_or_default(),
                to_portable_source_path_with(
                    Path::new(reference.source_path.as_deref().unwrap_or_default()),
                    Some(project_root),
                    built_in
                )
            )
        })
        .collect())
}

fn heal_catalog(
    project_root: &Path,
    built_in: Option<&Path>,
    dry_run: bool,
) -> Result<Vec<String>, InstallError> {
    let path = charter_yaml_path(project_root);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let document = read_document(&path)?;
    let references = catalog_reference_mappings(&document);
    let mut changes = Vec::new();
    for mapping in references {
        let reference = CatalogReference {
            id: mapping.get_string("id"),
            kind: mapping.get_string("kind"),
            source_path: mapping.get_string("source_path"),
        };
        if !catalog_ref_is_healable(&reference, built_in) {
            continue;
        }
        let old_value = reference.source_path.clone().unwrap_or_default();
        let new_value =
            to_portable_source_path_with(Path::new(&old_value), Some(project_root), built_in);
        changes.push(format!(
            "charter.yaml catalog[{}].source_path: {old_value} -> {new_value}",
            reference.id.as_deref().unwrap_or("?")
        ));
        if !dry_run {
            mapping.set_string("source_path", &new_value);
        }
    }
    if !changes.is_empty() && !dry_run {
        write_document(&path, &document)?;
    }
    Ok(changes)
}

fn manifest_entry_is_healable(entry: &ProfileManifestEntry, built_in: Option<&Path>) -> bool {
    let source_path = entry.source_path.as_str();
    if source_path.is_empty() || !is_absolute(source_path) {
        return false;
    }
    is_built_in_pack_path_with(Path::new(source_path), built_in)
}

fn healable_manifest(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Vec<String>, InstallError> {
    Ok(healable_manifest_entries(project_root, built_in)?
        .iter()
        .map(|entry| {
            format!(
                "agent_profiles_manifest.json[{}/{}].source_path: {} -> {}",
                entry.profile_urn,
                entry.tool_key,
                entry.source_path,
                to_portable_source_path_with(
                    Path::new(&entry.source_path),
                    Some(project_root),
                    built_in
                )
            )
        })
        .collect())
}

/// The manifest entries the heal would rewrite, in manifest order.
fn healable_manifest_entries(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Vec<ProfileManifestEntry>, InstallError> {
    if !manifest_path(project_root).exists() {
        return Ok(Vec::new());
    }
    let manifest = ProfileManifest::load(project_root)?;
    Ok(manifest
        .entries()
        .filter(|entry| manifest_entry_is_healable(entry, built_in))
        .cloned()
        .collect())
}

/// The healable catalog references, in document order.
fn healable_catalog_references(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Vec<CatalogReference>, InstallError> {
    let path = charter_yaml_path(project_root);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let document = read_document(&path)?;
    Ok(catalog_references(&document)
        .into_iter()
        .filter(|reference| catalog_ref_is_healable(reference, built_in))
        .collect())
}

/// `3.2.7_heal_provenance_paths.describe_leaks`: the read-only account of every
/// healable leak, in the Source's own `repr`quoting.
///
/// This is the single source of truth the `doctor provenance` leak check reads,
/// from exactly the classification `detect` uses, so a flagged leak is always one
/// `apply` (or a fresh compile) would fix — and vice versa. It never mutates.
pub fn describe_leaks(project_root: &Path) -> Result<Vec<String>, InstallError> {
    describe_leaks_with(project_root, None)
}

/// [`describe_leaks`] with an explicit built-in pack root.
pub fn describe_leaks_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Vec<String>, InstallError> {
    let mut descriptions = Vec::new();
    for reference in healable_catalog_references(project_root, built_in)? {
        descriptions.push(format!(
            "charter.yaml catalog[{}].source_path={}",
            reference.id.as_deref().unwrap_or("?"),
            sk_kernel::python_repr(reference.source_path.as_deref().unwrap_or_default())
        ));
    }
    for entry in healable_manifest_entries(project_root, built_in)? {
        descriptions.push(format!(
            "agent_profiles_manifest.json[{}/{}].source_path={}",
            entry.profile_urn,
            entry.tool_key,
            sk_kernel::python_repr(&entry.source_path)
        ));
    }
    Ok(descriptions)
}

fn heal_manifest(
    project_root: &Path,
    built_in: Option<&Path>,
    dry_run: bool,
) -> Result<Vec<String>, InstallError> {
    if !manifest_path(project_root).exists() {
        return Ok(Vec::new());
    }
    let mut manifest = ProfileManifest::load(project_root)?;
    let healable: Vec<ProfileManifestEntry> = manifest
        .entries()
        .filter(|entry| manifest_entry_is_healable(entry, built_in))
        .cloned()
        .collect();
    if healable.is_empty() {
        return Ok(Vec::new());
    }
    let mut changes = Vec::new();
    for entry in healable {
        let new_value = to_portable_source_path_with(
            Path::new(&entry.source_path),
            Some(project_root),
            built_in,
        );
        changes.push(format!(
            "agent_profiles_manifest.json[{}/{}].source_path: {} -> {new_value}",
            entry.profile_urn, entry.tool_key, entry.source_path
        ));
        if !dry_run {
            manifest.record(ProfileManifestEntry {
                source_path: new_value,
                ..entry
            });
        }
    }
    if !dry_run {
        manifest.save(project_root)?;
    }
    Ok(changes)
}

/// `3.2.7_heal_provenance_paths.detect`.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    detect_with(project_root, None)
}

/// [`detect`] with an explicit built-in pack root.
pub fn detect_with(project_root: &Path, built_in: Option<&Path>) -> Result<bool, InstallError> {
    Ok(!healable_catalog(project_root, built_in)?.is_empty()
        || !healable_manifest(project_root, built_in)?.is_empty())
}

/// `3.2.7_heal_provenance_paths.can_apply`.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    can_apply_with(project_root, None)
}

/// [`can_apply`] with an explicit built-in pack root.
pub fn can_apply_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Option<String>, InstallError> {
    if detect_with(project_root, built_in)? {
        return Ok(None);
    }
    Ok(Some(
        "no absolute built-in-pack source_path found in charter.yaml's catalog or \
         agent_profiles_manifest.json"
            .to_string(),
    ))
}

/// `3.2.7_heal_provenance_paths.apply`.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    apply_with(project_root, None)
}

/// [`apply`] with an explicit built-in pack root.
pub fn apply_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<MigrationOutcome, InstallError> {
    run(project_root, built_in, false)
}

/// `3.2.7_heal_provenance_paths.apply(dry_run=True)`: the same composition, no writes.
pub fn preview(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    preview_with(project_root, None)
}

/// [`preview`] with an explicit built-in pack root.
pub fn preview_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<MigrationOutcome, InstallError> {
    run(project_root, built_in, true)
}

/// The one composition both `apply` and `preview` use.
fn run(
    project_root: &Path,
    built_in: Option<&Path>,
    dry_run: bool,
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = heal_catalog(project_root, built_in, dry_run)?;
    changes.extend(heal_manifest(project_root, built_in, dry_run)?);
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// `4_0_0rc5_heal_template_set_provenance`: the previously excluded catalog rows
// ---------------------------------------------------------------------------

fn built_in_parts() -> Vec<String> {
    BUILT_IN_PARTS
        .iter()
        .map(|part| (*part).to_string())
        .collect()
}

/// The `built-in/...` suffix of a portable token, when it names the built-in tier.
fn portable_builtin_suffix(token: &str) -> Option<String> {
    if !token.starts_with("${") {
        return None;
    }
    let (_, suffix) = token.split_once("}/")?;
    let built_in = format!("{}/", BUILT_IN_PARTS[1]);
    suffix.starts_with(&built_in).then(|| suffix.to_string())
}

/// The expected `built-in/missions/<mission>/mission.yaml` suffix, when `mission`
/// is a plain directory name.
fn expected_mission_suffix(mission: Option<&str>) -> Option<String> {
    let mission = mission?;
    if mission.is_empty() || mission.contains('/') || mission.contains('\\') {
        return None;
    }
    if mission == "." || mission == ".." {
        return None;
    }
    Some(format!(
        "{}/missions/{mission}/{MISSION_CONFIG_FILENAME}",
        BUILT_IN_PARTS[1]
    ))
}

/// The portable token of a built-in mission's `mission.yaml`, when resolvable.
fn mission_token(mission: Option<&str>, built_in: Option<&Path>) -> Option<String> {
    expected_mission_suffix(mission)?;
    let missions_root = match built_in {
        Some(root) => root.to_path_buf(),
        None => sk_kernel::paths::get_built_in_pack_root().ok()?,
    }
    .join("missions");
    let source = missions_root.join(mission?).join(MISSION_CONFIG_FILENAME);
    let source = if source.is_file() {
        source
    } else {
        missions_root.join(mission?).join(MISSION_CONFIG_FILENAME)
    };
    let token = to_portable_source_path_with(&source, None, built_in);
    portable_builtin_suffix(&token).map(|_| token)
}

fn posix_parts(path: &str) -> Vec<String> {
    path.replace('\\', "/")
        .split('/')
        .map(str::to_string)
        .collect()
}

/// Whether `source_path` ends with `token_suffix` immediately below `packs/built-in`.
fn matches_canonical_source_shape(source_path: &str, token_suffix: &str) -> bool {
    let source_parts = posix_parts(source_path);
    let expected_parts = posix_parts(token_suffix);
    if source_parts.len() < expected_parts.len()
        || source_parts[source_parts.len() - expected_parts.len()..] != expected_parts[..]
    {
        return false;
    }
    let built_in_parts = built_in_parts();
    let built_in_index = source_parts.len() - expected_parts.len();
    if built_in_index + 1 < built_in_parts.len() {
        return false;
    }
    let root_start = built_in_index + 1 - built_in_parts.len();
    source_parts[root_start..=built_in_index] == built_in_parts[..]
}

/// Whether a URL names the canonical `spec-kitty/spec-kitty` repository.
fn is_spec_kitty_repository(value: &str) -> bool {
    let normalized = value.trim().to_lowercase();
    let normalized = match normalized.strip_prefix("git@github.com:") {
        Some(rest) => format!("ssh://github.com/{rest}"),
        None => normalized,
    };
    let Some((scheme, rest)) = normalized.split_once("://") else {
        return false;
    };
    if scheme != "https" && scheme != "ssh" {
        return false;
    }
    let (host, path) = match rest.split_once('/') {
        Some(parts) => parts,
        None => return false,
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    format!("{host}/{path}") == SPEC_KITTY_REPOSITORY
}

/// `(dev, ino, file-type)` of every existing component of `checkout_root/relative`.
///
/// `None` when a component is a symlink (or a non-directory in a non-final
/// position, or the final component is not a regular file); a missing component
/// ends the snapshot early, exactly like the Source.
#[cfg(unix)]
fn source_path_snapshot(checkout_root: &Path, relative: &Path) -> Option<Vec<(u64, u64, u32)>> {
    use std::os::unix::fs::MetadataExt;
    if !checkout_root.is_absolute() || relative.is_absolute() {
        return None;
    }
    let mut components: Vec<PathBuf> = Vec::new();
    for component in checkout_root.components() {
        match component {
            std::path::Component::RootDir => {}
            other => components.push(PathBuf::from(other.as_os_str())),
        }
    }
    for component in relative.components() {
        let part = component.as_os_str().to_string_lossy().to_string();
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        components.push(PathBuf::from(part));
    }
    let mut current = PathBuf::from("/");
    let mut snapshot = Vec::new();
    let last = components.len().saturating_sub(1);
    for (index, part) in components.iter().enumerate() {
        current.push(part);
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Some(snapshot);
            }
            Err(_) => return None,
        };
        let mode = metadata.mode();
        if metadata.file_type().is_symlink() {
            return None;
        }
        let is_final = index == last;
        if is_final && !metadata.is_file() {
            return None;
        }
        if !is_final && !metadata.is_dir() {
            return None;
        }
        snapshot.push((metadata.dev(), metadata.ino(), mode & 0o170000));
    }
    Some(snapshot)
}

#[cfg(not(unix))]
fn source_path_snapshot(checkout_root: &Path, relative: &Path) -> Option<Vec<(u64, u64, u32)>> {
    if !checkout_root.is_absolute() || relative.is_absolute() {
        return None;
    }
    let mut current = checkout_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Some(Vec::new());
            }
            Err(_) => return None,
        };
        if metadata.file_type().is_symlink() {
            return None;
        }
    }
    Some(Vec::new())
}

/// The reference bounds both former-checkout Git reads with `timeout=2`.
const CHECKOUT_GIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Open the candidate checkout exactly as the reference's
/// `git -C <checkout_root> rev-parse --show-toplevel` leg does: the shared Git
/// adapter resolves the work-tree root of this path, and a failure to read it is
/// "identity unproven" rather than an error the heal would swallow.
fn checkout_repository(checkout_root: &Path) -> Option<sk_git::GitRepository> {
    let options = sk_git::GitOptions {
        timeout: CHECKOUT_GIT_TIMEOUT,
        ..sk_git::GitOptions::default()
    };
    sk_git::GitRepository::open_with_options(checkout_root, &options).ok()
}

/// The checkout's canonical origin: `git remote get-url origin`, trimmed.
///
/// This is the reference's own read (`subprocess.run(["git", "-C", root,
/// "remote", "get-url", "origin"], check=True, timeout=2)`), routed through the
/// shared local Git adapter, so `url.<base>.insteadOf` rewrites apply and the
/// network is never contacted. The reference's `check=True` turns a *missing*
/// remote into a caught `CalledProcessError`, so `Ok(None)` and any other error
/// both mean "origin identity unproven" — never a weaker proof.
fn canonical_origin(repository: &sk_git::GitRepository) -> Option<String> {
    let remote = sk_git::RemoteName::parse("origin").ok()?;
    repository.remote_url(&remote).ok().flatten()
}

fn checkout_tracks_mission(
    checkout_root: &Path,
    relative_source: &Path,
    initial_snapshot: &[(u64, u64, u32)],
) -> bool {
    let Some(pyproject) = std::fs::read_to_string(checkout_root.join("pyproject.toml")).ok() else {
        return false;
    };
    let Ok(document) = pyproject.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    let project = document.get("project");
    let name = project
        .and_then(|project| project.get("name"))
        .and_then(|name| name.as_str());
    if name != Some(SPEC_KITTY_PROJECT_NAME) {
        return false;
    }
    let repository = project
        .and_then(|project| project.get("urls"))
        .and_then(|urls| urls.as_table_like())
        .and_then(|urls| {
            urls.iter()
                .find(|(key, _)| key.to_lowercase() == "repository")
                .map(|(_, value)| value)
        })
        .and_then(|value| value.as_str());
    let Some(repository) = repository else {
        return false;
    };
    if !is_spec_kitty_repository(repository) {
        return false;
    }

    let Some(repository_handle) = checkout_repository(checkout_root) else {
        return false;
    };
    let Ok(resolved_root) = sk_kernel::paths::resolve_path_non_strict(checkout_root) else {
        return false;
    };
    if repository_handle.root() != resolved_root.as_path() {
        return false;
    }
    let Some(origin) = canonical_origin(&repository_handle) else {
        return false;
    };
    if !is_spec_kitty_repository(&origin) {
        return false;
    }

    let before_query = source_path_snapshot(checkout_root, relative_source);
    if before_query.as_deref() != Some(initial_snapshot) {
        return false;
    }
    let literal = relative_source.to_string_lossy().replace('\\', "/");
    let specs: [&std::ffi::OsStr; 1] = [std::ffi::OsStr::new(&literal)];
    let Ok(records) = repository_handle.index_entries(&specs) else {
        return false;
    };
    let after_query = source_path_snapshot(checkout_root, relative_source);
    if after_query.as_deref() != before_query.as_deref() {
        return false;
    }
    if records.len() != 1 {
        return false;
    }
    let record = &records[0];
    record.path().as_os_str() == std::ffi::OsStr::new(&literal)
        && matches!(record.mode(), "100644" | "100755")
        && record.stage() == 0
}

/// Whether a matching absolute path is proven to come from a former Spec Kitty checkout.
fn has_former_checkout_proof(source_path: &str, token_suffix: &str) -> bool {
    let source = Path::new(source_path);
    let mut relative_parts: Vec<String> = built_in_parts();
    let mut suffix_parts = posix_parts(token_suffix);
    if !suffix_parts.is_empty() {
        suffix_parts.remove(0);
    }
    relative_parts.extend(suffix_parts);
    let relative_source = PathBuf::from(relative_parts.join("/"));
    let mut checkout_root = source.to_path_buf();
    for _ in &relative_parts {
        match checkout_root.parent() {
            Some(parent) => checkout_root = parent.to_path_buf(),
            None => return false,
        }
    }
    let Some(initial_snapshot) = source_path_snapshot(&checkout_root, &relative_source) else {
        return false;
    };
    let Ok(resolved_root) = sk_kernel::paths::resolve_path_non_strict(&checkout_root) else {
        return false;
    };
    let Ok(resolved_source) = sk_kernel::paths::resolve_path_non_strict(source) else {
        return false;
    };
    let expected_source =
        match sk_kernel::paths::resolve_path_non_strict(&resolved_root.join(&relative_source)) {
            Ok(path) => path,
            Err(_) => return false,
        };
    if resolved_source != expected_source {
        return false;
    }
    checkout_tracks_mission(&checkout_root, &relative_source, &initial_snapshot)
}

fn matches_mission_source(source_path: &str, token: &str, built_in: Option<&Path>) -> bool {
    if !is_absolute(source_path) {
        return false;
    }
    let Some(token_suffix) = portable_builtin_suffix(token) else {
        return false;
    };
    if !matches_canonical_source_shape(source_path, &token_suffix) {
        return false;
    }
    is_built_in_pack_path_with(Path::new(source_path), built_in)
        || has_former_checkout_proof(source_path, &token_suffix)
}

/// One healable `template_set` catalog row: the reference id, its old and new path.
pub struct TemplateSetHeal {
    /// The catalog reference id.
    pub id: String,
    /// The authored absolute path.
    pub source_path: String,
    /// The portable token it becomes.
    pub token: String,
}

fn healable_template_set_references(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Vec<TemplateSetHeal>, InstallError> {
    let path = charter_yaml_path(project_root);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    // The reference's `_load_charter_document`: an unreadable charter is a typed
    // `MigrationStateUnreadableError`, never "nothing to heal".
    let document = read_unreadable_as_state(&path)?;
    let Some(catalog) = document.root().get_mapping("catalog") else {
        return Ok(Vec::new());
    };
    let mission = catalog.get_string("mission");
    let Some(template_set) = catalog.get_string("template_set") else {
        return Ok(Vec::new());
    };
    if template_set.is_empty() {
        return Ok(Vec::new());
    }
    let expected_id = format!("TEMPLATE_SET:{template_set}");
    let Some(token) = mission_token(mission.as_deref(), built_in) else {
        return Ok(Vec::new());
    };
    let mut healable = Vec::new();
    for reference in catalog_references(&document) {
        if reference.kind.as_deref() != Some(TEMPLATE_SET_KIND) {
            continue;
        }
        if reference.id.as_deref() != Some(expected_id.as_str()) {
            continue;
        }
        let Some(source_path) = reference.source_path.clone() else {
            continue;
        };
        if matches_mission_source(&source_path, &token, built_in) {
            healable.push(TemplateSetHeal {
                id: expected_id.clone(),
                source_path,
                token: token.clone(),
            });
        }
    }
    Ok(healable)
}

/// Stale built-in template-set source paths the heal would rewrite (read-only).
pub fn describe_template_set_leaks(project_root: &Path) -> Result<Vec<String>, InstallError> {
    describe_template_set_leaks_with(project_root, None)
}

/// [`describe_template_set_leaks`] with an explicit built-in pack root.
pub fn describe_template_set_leaks_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Vec<String>, InstallError> {
    Ok(healable_template_set_references(project_root, built_in)?
        .into_iter()
        .map(|heal| {
            format!(
                "charter.yaml catalog[{}].source_path={}",
                heal.id,
                sk_kernel::python_repr(&heal.source_path)
            )
        })
        .collect())
}

/// Candidate legacy template paths whose checkout identity is unproven (read-only).
pub fn describe_template_set_ambiguities(project_root: &Path) -> Result<Vec<String>, InstallError> {
    describe_template_set_ambiguities_with(project_root, None)
}

/// [`describe_template_set_ambiguities`] with an explicit built-in pack root.
pub fn describe_template_set_ambiguities_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Vec<String>, InstallError> {
    let path = charter_yaml_path(project_root);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let document = read_unreadable_as_state(&path)?;
    let Some(catalog) = document.root().get_mapping("catalog") else {
        return Ok(Vec::new());
    };
    let Some(suffix) = expected_mission_suffix(catalog.get_string("mission").as_deref()) else {
        return Ok(Vec::new());
    };
    let Some(template_set) = catalog.get_string("template_set") else {
        return Ok(Vec::new());
    };
    if template_set.is_empty() {
        return Ok(Vec::new());
    }
    let expected_id = format!("TEMPLATE_SET:{template_set}");
    let mut ambiguous = Vec::new();
    for reference in catalog_references(&document) {
        if reference.kind.as_deref() != Some(TEMPLATE_SET_KIND)
            || reference.id.as_deref() != Some(expected_id.as_str())
        {
            continue;
        }
        let Some(source_path) = reference.source_path.as_deref() else {
            continue;
        };
        if !is_absolute(source_path) {
            continue;
        }
        let synthetic_token = format!("${{SPEC_KITTY_PACKS_ROOT}}/{suffix}");
        if matches_mission_source(source_path, &synthetic_token, built_in) {
            continue;
        }
        let source_parts = posix_parts(source_path);
        let suffix_parts = posix_parts(&suffix);
        if source_parts.len() >= suffix_parts.len()
            && source_parts[source_parts.len() - suffix_parts.len()..] == suffix_parts[..]
        {
            ambiguous.push(format!(
                "ambiguous template-set provenance (checkout identity cannot be verified; path not healed and left unchanged): \
                 charter.yaml catalog[{expected_id}].source_path={}",
                sk_kernel::python_repr(source_path)
            ));
        }
    }
    Ok(ambiguous)
}

/// `4_0_0rc5_heal_template_set_provenance.detect`.
pub fn detect_template_set(project_root: &Path) -> Result<bool, InstallError> {
    detect_template_set_with(project_root, None)
}

/// [`detect_template_set`] with an explicit built-in pack root.
pub fn detect_template_set_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<bool, InstallError> {
    Ok(!healable_template_set_references(project_root, built_in)?.is_empty())
}

/// `4_0_0rc5_heal_template_set_provenance.can_apply`.
pub fn can_apply_template_set(project_root: &Path) -> Result<Option<String>, InstallError> {
    can_apply_template_set_with(project_root, None)
}

/// [`can_apply_template_set`] with an explicit built-in pack root.
pub fn can_apply_template_set_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<Option<String>, InstallError> {
    if detect_template_set_with(project_root, built_in)? {
        return Ok(None);
    }
    Ok(Some(
        "no stale built-in template_set source_path found in charter.yaml".to_string(),
    ))
}

/// `4_0_0rc5_heal_template_set_provenance.apply`.
pub fn apply_template_set(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    apply_template_set_with(project_root, None)
}

/// [`apply_template_set`] with an explicit built-in pack root.
pub fn apply_template_set_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<MigrationOutcome, InstallError> {
    run_template_set(project_root, built_in, false)
}

/// `4_0_0rc5_heal_template_set_provenance.apply(dry_run=True)`.
pub fn preview_template_set(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    preview_template_set_with(project_root, None)
}

/// [`preview_template_set`] with an explicit built-in pack root.
pub fn preview_template_set_with(
    project_root: &Path,
    built_in: Option<&Path>,
) -> Result<MigrationOutcome, InstallError> {
    run_template_set(project_root, built_in, true)
}

/// The one composition both `apply_template_set` and the preview use.
fn run_template_set(
    project_root: &Path,
    built_in: Option<&Path>,
    dry_run: bool,
) -> Result<MigrationOutcome, InstallError> {
    let path = charter_yaml_path(project_root);
    if !path.is_file() {
        return Ok(MigrationOutcome::skipped());
    }
    let healable = healable_template_set_references(project_root, built_in)?;
    if healable.is_empty() {
        return Ok(MigrationOutcome::skipped());
    }
    let document = read_unreadable_as_state(&path)?;
    let mut changes = Vec::new();
    for mapping in catalog_reference_mappings(&document) {
        let Some(id) = mapping.get_string("id") else {
            continue;
        };
        let Some(heal) = healable.iter().find(|heal| heal.id == id) else {
            continue;
        };
        changes.push(format!(
            "charter.yaml catalog[{id}].source_path: {} -> {}",
            heal.source_path, heal.token
        ));
        if !dry_run {
            mapping.set_string("source_path", &heal.token);
        }
    }
    if !changes.is_empty() && !dry_run {
        write_document(&path, &document)?;
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// The provenance migrations this module implements.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[
    NativeMigration {
        id: "3.2.7_heal_provenance_paths",
        detect,
        can_apply,
        apply,
    },
    NativeMigration {
        id: "4_0_0rc5_heal_template_set_provenance",
        detect: detect_template_set,
        can_apply: can_apply_template_set,
        apply: apply_template_set,
    },
];
