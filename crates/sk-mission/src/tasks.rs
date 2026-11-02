//! `tasks.md` outline parsing and dependency finalization.
//!
//! The outline parser mirrors the pinned reference
//! (`specify_cli/core/dependency_parser.py` and
//! `cli/commands/agent/tasks_outline.py`): it recognises the three explicit
//! dependency declaration formats, bounds the final work-package section so
//! trailing prose cannot become a false dependency, and resolves structured
//! task rows to their owning work package.
//!
//! Finalization is planned first and written second. Every validation gate
//! (coverage, frontmatter conflict, self/unknown/cycle) runs before the first
//! byte is written; a failed multi-file write restores every prior byte. Only
//! the `dependencies` value changes in a work-package prompt — comments, key
//! order, indentation, line endings, and the Markdown body are preserved.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde_yaml_ng::{Mapping, Value};
use sk_kernel::locks::FileLock;
use sk_kernel::{WpId, atomic_write, read_bounded};
use thiserror::Error;

use crate::create::MissionWriteObserver;
use crate::frontmatter::split_frontmatter;
use crate::paths::{TASKS_DIR_NAME, TASKS_FILE_NAME};
use crate::work_package::DEFAULT_WORK_PACKAGE_BYTE_LIMIT;
use crate::yaml_patch::{
    PatchError, count_top_level_field, decode_mapping, sequence_of, set_top_level_sequence,
};

/// Default byte budget for one `tasks.md` read.
pub const DEFAULT_TASKS_MD_BYTE_LIMIT: u64 = 4 * 1024 * 1024;

const LOCK_DIR: &str = "spec-kitty-locks";
const WRITE_LOCK_FILE: &str = "mission-write.lock";
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// One work-package section of `tasks.md`.
#[derive(Clone, Debug)]
pub struct TasksSection {
    wp_id: WpId,
    heading: String,
    body: String,
    dependencies: Vec<WpId>,
    task_ids: Vec<String>,
}

impl TasksSection {
    /// Canonical work-package id.
    pub fn wp_id(&self) -> &WpId {
        &self.wp_id
    }

    /// Heading title as authored.
    pub fn heading(&self) -> &str {
        &self.heading
    }

    /// Section body (after the heading line, before the next section).
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Declared dependencies, in declaration order.
    pub fn dependencies(&self) -> &[WpId] {
        &self.dependencies
    }

    /// Structured task ids named by checkbox, pipe-table, or `Subtasks:` rows.
    pub fn task_ids(&self) -> &[String] {
        &self.task_ids
    }
}

/// A parsed `tasks.md` outline.
#[derive(Clone, Debug)]
pub struct TasksOutline {
    sections: Vec<TasksSection>,
    dependencies: BTreeMap<WpId, Vec<WpId>>,
}

impl TasksOutline {
    /// Read and parse a `tasks.md` file under the default byte budget.
    pub fn from_file(path: &Path) -> Result<Self, TasksAuthoringError> {
        let bytes = read_bounded(path, DEFAULT_TASKS_MD_BYTE_LIMIT)
            .map_err(|source| map_read_error(path, source))?;
        let source =
            std::str::from_utf8(&bytes).map_err(|error| TasksAuthoringError::InvalidUtf8 {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })?;
        Self::parse(source)
    }

    /// Parse already-read `tasks.md` content.
    pub fn parse(source: &str) -> Result<Self, TasksAuthoringError> {
        let headings = wp_headings(source);
        let mut sections = Vec::with_capacity(headings.len());
        let mut seen = BTreeSet::new();
        for (index, heading) in headings.iter().enumerate() {
            if !seen.insert(heading.wp_id.clone()) {
                return Err(TasksAuthoringError::DuplicateSection {
                    wp: heading.wp_id.clone(),
                });
            }
            let end = match headings.get(index + 1) {
                Some(next) => next.line_start,
                None => final_section_end(source, heading.body_start),
            };
            let body = &source[heading.body_start..end];
            let dependencies = parse_section_dependencies(body);
            let task_ids = extract_task_ids(body);
            sections.push(TasksSection {
                wp_id: heading.wp_id.clone(),
                heading: heading.title.clone(),
                body: body.to_owned(),
                dependencies: dependencies.clone(),
                task_ids,
            });
        }
        let dependencies = sections
            .iter()
            .map(|section| (section.wp_id.clone(), section.dependencies.clone()))
            .collect();
        Ok(Self {
            sections,
            dependencies,
        })
    }

    /// Sections in authored order.
    pub fn sections(&self) -> &[TasksSection] {
        &self.sections
    }

    /// Parsed dependency declarations keyed by work-package id.
    pub fn dependencies(&self) -> &BTreeMap<WpId, Vec<WpId>> {
        &self.dependencies
    }

    /// Work-package ids whose structured rows name `task_id`, sorted.
    pub fn owners_of_task(&self, task_id: &str) -> Vec<String> {
        let normalized = task_id.trim().to_ascii_uppercase();
        let mut owners: Vec<String> = self
            .sections
            .iter()
            .filter(|section| section.task_ids.iter().any(|id| id == &normalized))
            .map(|section| section.wp_id.to_string())
            .collect();
        owners.sort();
        owners.dedup();
        owners
    }
}

/// Parse only the dependency declarations, matching the Python parser API.
pub fn parse_dependencies_from_tasks_md(
    source: &str,
) -> Result<BTreeMap<String, Vec<WpId>>, TasksAuthoringError> {
    let outline = TasksOutline::parse(source)?;
    Ok(outline
        .sections
        .iter()
        .map(|section| (section.wp_id.to_string(), section.dependencies.clone()))
        .collect())
}

struct Heading {
    wp_id: WpId,
    title: String,
    line_start: usize,
    body_start: usize,
}

fn wp_headings(source: &str) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut offset = 0usize;
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        let title = if let Some(title) = content.strip_prefix("## ") {
            title
        } else if let Some(title) = content.strip_prefix("### ") {
            title
        } else {
            continue;
        };
        if let Some(wp_id) = match_wp_section_id(title.trim()) {
            headings.push(Heading {
                wp_id,
                title: title.trim().to_owned(),
                line_start: start,
                body_start: offset,
            });
        }
    }
    headings
}

fn final_section_end(source: &str, body_start: usize) -> usize {
    let mut offset = body_start;
    for line in source[body_start..].split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        let Some(title) = content.strip_prefix("## ") else {
            continue;
        };
        let title = title.trim();
        if match_wp_section_id(title).is_some() || is_dependencies_heading(title) {
            continue;
        }
        return start;
    }
    source.len()
}

/// Canonical work-package heading matcher.
fn match_wp_section_id(title: &str) -> Option<WpId> {
    let lower = title.to_ascii_lowercase();
    if let Some(suffix) = lower
        .strip_prefix("work package ")
        .map(|_| &title["Work Package ".len()..])
    {
        let digits: String = suffix.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() || digits.len() > 2 {
            return None;
        }
        let remainder = &suffix[digits.len()..];
        if remainder
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        if !remainder.is_empty()
            && !remainder.chars().next().is_some_and(|character| {
                character.is_whitespace() || matches!(character, ':' | '-' | '—')
            })
        {
            return None;
        }
        let number: u32 = digits.parse().ok()?;
        return WpId::from_str(&format!("WP{number:02}")).ok();
    }

    let (prefix, rest) = title.split_at_checked(2)?;
    if !prefix.eq_ignore_ascii_case("WP") {
        return None;
    }
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let remainder = &rest[digits.len()..];
    if remainder
        .chars()
        .next()
        .is_some_and(|character| character.is_alphanumeric() || character == '_')
    {
        return None;
    }
    WpId::from_str(&format!("WP{digits}")).ok()
}

fn is_dependencies_heading(title: &str) -> bool {
    let mut text = title.trim();
    let hashes = text
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if !(1..=4).contains(&hashes) {
        return false;
    }
    text = text[hashes..].trim_start();
    if let Some(rest) = text.strip_prefix("**") {
        text = rest;
    } else if let Some(rest) = text.strip_prefix('*') {
        text = rest;
    }
    let lower = text.to_ascii_lowercase();
    if !lower.starts_with("dependencies") {
        return false;
    }
    let mut rest = &text["dependencies".len()..];
    if let Some(stripped) = rest.strip_prefix("**") {
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix('*') {
        rest = stripped;
    }
    rest.trim().is_empty()
}

fn parse_section_dependencies(body: &str) -> Vec<WpId> {
    let mut ids: Vec<WpId> = Vec::new();
    let mut in_dependency_bullets = false;

    for raw_line in body.lines() {
        let line = raw_line.trim_end_matches('\r');
        if in_dependency_bullets {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('#') {
                in_dependency_bullets = false;
            } else if let Some(id) = bullet_wp_id(trimmed) {
                push_unique(&mut ids, id);
                continue;
            } else {
                in_dependency_bullets = false;
            }
        }

        if is_dependencies_heading(trimmed_heading(line)) {
            in_dependency_bullets = true;
            continue;
        }

        if let Some(value) = depends_on_value(line) {
            for id in parse_wp_list(value).0 {
                if let Ok(id) = WpId::from_str(&id) {
                    push_unique(&mut ids, id);
                }
            }
        }

        if let Some(value) = colon_dependency_value(line) {
            for id in parse_colon_value(value) {
                push_unique(&mut ids, id);
            }
        }
    }

    ids
}

fn trimmed_heading(line: &str) -> &str {
    line.trim()
}

fn depends_on_value(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    if let Some(stripped) = rest.strip_prefix("- ") {
        rest = stripped.trim_start();
    } else if let Some(stripped) = rest.strip_prefix("* ") {
        rest = stripped.trim_start();
    }
    let lower = rest.to_ascii_lowercase();
    for prefix in ["depends on ", "depend on "] {
        if lower.starts_with(prefix) {
            return Some(&rest[prefix.len()..]);
        }
    }
    None
}

fn colon_dependency_value(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }

    for (pipe, _) in line.match_indices('|') {
        let mut rest = line[pipe + 1..].trim_start();
        rest = rest.trim_start_matches('*');
        let lower = rest.to_ascii_lowercase();
        if lower.starts_with("dependencies") {
            rest = &rest["dependencies".len()..];
            rest = rest.trim_start_matches('*');
            rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix(':') {
                return Some(value.split('|').next().unwrap_or(value));
            }
        }
    }

    let mut rest = line.trim_start();
    if let Some(stripped) = rest.strip_prefix("- ") {
        rest = stripped.trim_start();
    } else if let Some(stripped) = rest.strip_prefix("* ") {
        rest = stripped.trim_start();
    }
    rest = rest.trim_start_matches('*');
    let lower = rest.to_ascii_lowercase();
    if !lower.starts_with("dependencies") {
        return None;
    }
    rest = &rest["dependencies".len()..];
    rest = rest.trim_start_matches('*');
    rest = rest.trim_start();
    rest.strip_prefix(':')
        .map(|value| value.split('|').next().unwrap_or(value))
}

fn bullet_wp_id(line: &str) -> Option<WpId> {
    let rest = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))?;
    let rest = rest.trim_start();
    let (id, _) = parse_wp_at(rest)?;
    WpId::from_str(id).ok()
}

fn parse_colon_value(value: &str) -> Vec<WpId> {
    let stripped = value.trim();
    let lower = stripped.to_ascii_lowercase();
    if stripped.is_empty()
        || lower == "[]"
        || lower.starts_with("none")
        || lower.starts_with("no deps")
        || lower.starts_with("no dependencies")
    {
        return Vec::new();
    }
    if let Some(rest) = depends_on_value(stripped) {
        return parse_wp_list(rest)
            .0
            .into_iter()
            .filter_map(|id| WpId::from_str(&id).ok())
            .collect();
    }
    let (ids, remainder) = parse_wp_list(stripped);
    if ids.is_empty() {
        return Vec::new();
    }
    let remainder = remainder.trim_start();
    if remainder.is_empty()
        || remainder
            .chars()
            .next()
            .is_some_and(|character| matches!(character, '.' | ';' | '(' | '#'))
    {
        ids.into_iter()
            .filter_map(|id| WpId::from_str(&id).ok())
            .collect()
    } else {
        Vec::new()
    }
}

fn parse_wp_list(value: &str) -> (Vec<String>, &str) {
    let mut ids = Vec::new();
    let mut rest = value.trim_start();
    while let Some((id, after)) = parse_wp_at(rest) {
        ids.push(id.to_owned());
        rest = after.trim_start();
        if let Some(after_comma) = rest.strip_prefix(',') {
            rest = after_comma.trim_start();
            continue;
        }
        break;
    }
    (ids, rest)
}

fn parse_wp_at(value: &str) -> Option<(&str, &str)> {
    let (prefix, rest) = value.split_at_checked(2)?;
    if !prefix.eq_ignore_ascii_case("WP") {
        return None;
    }
    let digits: usize = rest
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits != 2 {
        return None;
    }
    let end = 2 + digits;
    let after = &value[end..];
    if after
        .chars()
        .next()
        .is_some_and(|character| character.is_alphanumeric() || character == '_')
    {
        return None;
    }
    Some((&value[..end], after))
}

fn push_unique(ids: &mut Vec<WpId>, id: WpId) {
    if !ids.contains(&id) {
        ids.push(id);
    }
}

fn extract_task_ids(body: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for raw_line in body.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some(task) = checkbox_task_id(line) {
            push_unique_string(&mut ids, task);
        }
        if line.contains('|') {
            for cell in line.split('|') {
                let cell = cell.trim();
                if is_task_id(cell) {
                    push_unique_string(&mut ids, cell.to_ascii_uppercase());
                }
            }
        }
        if let Some(values) = inline_subtasks(line) {
            for value in values {
                if value.starts_with('T') {
                    push_unique_string(&mut ids, value);
                }
            }
        }
    }
    ids
}

fn checkbox_task_id(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('-')?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let mut chars = rest.chars();
    if !matches!(chars.next(), Some(' ' | 'x' | 'X')) {
        return None;
    }
    let rest = chars.as_str().strip_prefix(']')?;
    let rest = rest.trim_start();
    let task = rest
        .split(|character: char| !character.is_ascii_alphanumeric())
        .next()
        .filter(|value| is_task_id(value))?;
    Some(task.to_ascii_uppercase())
}

fn inline_subtasks(line: &str) -> Option<Vec<String>> {
    let lower = line.to_ascii_lowercase();
    let marker = lower.find("subtasks")?;
    let after = &line[marker + "subtasks".len()..];
    let after = after.trim_start_matches('*').trim_start();
    let after = after.strip_prefix(':')?;
    let mut values = Vec::new();
    for part in after.split(',') {
        let candidate: String = part
            .trim()
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric())
            .collect();
        if candidate.len() >= 2
            && (candidate[..1].eq_ignore_ascii_case("T")
                || candidate[..1].eq_ignore_ascii_case("W"))
            && candidate[1..].bytes().all(|byte| byte.is_ascii_digit())
        {
            values.push(candidate.to_ascii_uppercase());
        }
    }
    (!values.is_empty()).then_some(values)
}

fn is_task_id(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    upper.starts_with('T')
        && upper.len() > 1
        && upper[1..].bytes().all(|byte| byte.is_ascii_digit())
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

/// One planned dependency rewrite.
#[derive(Clone, Debug)]
pub struct DependencyUpdate {
    wp_id: WpId,
    path: PathBuf,
    original: Vec<u8>,
    updated: Vec<u8>,
    dependencies: Vec<WpId>,
}

impl DependencyUpdate {
    /// Work package whose prompt changes.
    pub fn wp_id(&self) -> &WpId {
        &self.wp_id
    }

    /// Prompt file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Prior bytes, used for stale-plan checks and rollback.
    pub fn original_bytes(&self) -> &[u8] {
        &self.original
    }

    /// New bytes.
    pub fn updated_bytes(&self) -> &[u8] {
        &self.updated
    }

    /// Dependencies that will be persisted.
    pub fn dependencies(&self) -> &[WpId] {
        &self.dependencies
    }
}

/// A validated, side-effect-free finalization plan.
#[derive(Clone, Debug)]
pub struct DependencyFinalizePlan {
    tasks_dir: PathBuf,
    updates: Vec<DependencyUpdate>,
    preserved_wps: Vec<String>,
    unchanged_wps: Vec<String>,
    effective: BTreeMap<WpId, Vec<WpId>>,
}

impl DependencyFinalizePlan {
    /// Compute the finalization plan. Reads only; never writes.
    pub fn compute(tasks_md: &Path, tasks_dir: &Path) -> Result<Self, TasksAuthoringError> {
        if fs::symlink_metadata(tasks_md).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(TasksAuthoringError::SymlinkedEntry {
                path: tasks_md.to_path_buf(),
            });
        }
        if fs::symlink_metadata(tasks_dir).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(TasksAuthoringError::SymlinkedEntry {
                path: tasks_dir.to_path_buf(),
            });
        }
        let metadata = fs::metadata(tasks_md).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                TasksAuthoringError::MissingTasksFile {
                    path: tasks_md.to_path_buf(),
                }
            } else {
                TasksAuthoringError::Io {
                    action: "inspect",
                    path: tasks_md.to_path_buf(),
                    source,
                }
            }
        })?;
        if !metadata.is_file() {
            return Err(TasksAuthoringError::MissingTasksFile {
                path: tasks_md.to_path_buf(),
            });
        }
        match fs::metadata(tasks_dir) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(TasksAuthoringError::MissingTasksDirectory {
                    path: tasks_dir.to_path_buf(),
                });
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(TasksAuthoringError::MissingTasksDirectory {
                    path: tasks_dir.to_path_buf(),
                });
            }
            Err(source) => {
                return Err(TasksAuthoringError::Io {
                    action: "inspect",
                    path: tasks_dir.to_path_buf(),
                    source,
                });
            }
        }

        let bytes = read_bounded(tasks_md, DEFAULT_TASKS_MD_BYTE_LIMIT)
            .map_err(|source| map_read_error(tasks_md, source))?;
        let source =
            std::str::from_utf8(&bytes).map_err(|error| TasksAuthoringError::InvalidUtf8 {
                path: tasks_md.to_path_buf(),
                detail: error.to_string(),
            })?;
        let outline = TasksOutline::parse(source)?;

        let files = discover_work_packages(tasks_dir)?;
        let expected: BTreeSet<WpId> = files.keys().cloned().collect();
        let parsed: BTreeSet<WpId> = outline.dependencies.keys().cloned().collect();
        let missing_sections: Vec<String> = expected
            .difference(&parsed)
            .map(ToString::to_string)
            .collect();
        let extra_sections: Vec<String> = parsed
            .difference(&expected)
            .map(ToString::to_string)
            .collect();
        if !missing_sections.is_empty() || !extra_sections.is_empty() {
            return Err(TasksAuthoringError::Coverage {
                missing_sections,
                extra_sections,
            });
        }

        let mut existing: BTreeMap<WpId, WorkPackageFrontmatter> = BTreeMap::new();
        for (wp_id, path) in &files {
            existing.insert(wp_id.clone(), read_frontmatter(path, wp_id)?);
        }

        let mut effective = BTreeMap::new();
        let mut preserved_wps = Vec::new();
        let mut unchanged_wps = Vec::new();
        let mut updates = Vec::new();

        for wp_id in expected {
            let parsed_deps = outline
                .dependencies
                .get(&wp_id)
                .cloned()
                .unwrap_or_default();
            let current = existing
                .get(&wp_id)
                .expect("coverage guarantees a frontmatter entry");
            if !current.dependencies.is_empty()
                && !parsed_deps.is_empty()
                && set_of(&current.dependencies) != set_of(&parsed_deps)
            {
                return Err(TasksAuthoringError::DependencyConflict {
                    wp: wp_id.to_string(),
                    existing: current
                        .dependencies
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    parsed: parsed_deps.iter().map(ToString::to_string).collect(),
                });
            }

            let resolved = if parsed_deps.is_empty() && !current.dependencies.is_empty() {
                preserved_wps.push(wp_id.to_string());
                current.dependencies.clone()
            } else {
                parsed_deps
            };
            effective.insert(wp_id.clone(), resolved.clone());

            if resolved != current.dependencies {
                let values: Vec<String> = resolved.iter().map(ToString::to_string).collect();
                let updated = set_top_level_sequence(&current.source, "dependencies", &values)
                    .map_err(|source| map_patch_error(&current.path, source))?
                    .ok_or_else(|| TasksAuthoringError::InvalidFrontmatter {
                        path: current.path.clone(),
                        detail: "dependencies differ but the text patch was a no-op".to_owned(),
                    })?;
                updates.push(DependencyUpdate {
                    wp_id: wp_id.clone(),
                    path: current.path.clone(),
                    original: current.source.as_bytes().to_vec(),
                    updated: updated.into_bytes(),
                    dependencies: resolved,
                });
            } else if !preserved_wps
                .iter()
                .any(|preserved| preserved.as_str() == wp_id.as_str())
            {
                unchanged_wps.push(wp_id.to_string());
            }
        }

        validate_dependency_graph(&effective)?;

        Ok(Self {
            tasks_dir: tasks_dir.to_path_buf(),
            updates,
            preserved_wps,
            unchanged_wps,
            effective,
        })
    }

    /// Work packages whose prompt will change, ascending.
    pub fn updated_wps(&self) -> Vec<String> {
        self.updates
            .iter()
            .map(|update| update.wp_id.to_string())
            .collect()
    }

    /// Work packages whose dependencies were preserved from frontmatter.
    pub fn preserved_wps(&self) -> Vec<&str> {
        self.preserved_wps.iter().map(String::as_str).collect()
    }

    /// Work packages that needed no change.
    pub fn unchanged_wps(&self) -> Vec<&str> {
        self.unchanged_wps.iter().map(String::as_str).collect()
    }

    /// The effective dependency graph that will be on disk after apply.
    pub fn effective_dependencies(&self) -> &BTreeMap<WpId, Vec<WpId>> {
        &self.effective
    }

    /// Planned updates.
    pub fn updates(&self) -> &[DependencyUpdate] {
        &self.updates
    }

    /// Apply the plan, returning what changed.
    pub fn apply(&self, tasks_dir: &Path) -> Result<DependencyApplyReceipt, TasksAuthoringError> {
        self.apply_with_observer(tasks_dir, &mut ())
    }

    /// Apply with a write observer, used by recovery and failure-injection tests.
    pub fn apply_with_observer(
        &self,
        tasks_dir: &Path,
        observer: &mut dyn MissionWriteObserver,
    ) -> Result<DependencyApplyReceipt, TasksAuthoringError> {
        if tasks_dir != self.tasks_dir {
            return Err(TasksAuthoringError::StalePlan {
                path: tasks_dir.to_path_buf(),
                detail: "apply was given a different tasks directory".to_owned(),
            });
        }
        if fs::symlink_metadata(tasks_dir).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(TasksAuthoringError::SymlinkedEntry {
                path: tasks_dir.to_path_buf(),
            });
        }
        if self.updates.is_empty() {
            return Ok(DependencyApplyReceipt {
                updated: Vec::new(),
                preserved: self.preserved_wps.clone(),
                unchanged: self.unchanged_wps.clone(),
            });
        }

        let root = project_root_for(tasks_dir)?;
        let lock_path = root.join(".kittify").join(LOCK_DIR).join(WRITE_LOCK_FILE);
        let _lock = FileLock::acquire(&lock_path, LOCK_TIMEOUT).map_err(|source| {
            TasksAuthoringError::Lock {
                path: lock_path.clone(),
                source,
            }
        })?;

        for update in &self.updates {
            let current = fs::read(update.path()).map_err(|source| TasksAuthoringError::Io {
                action: "read",
                path: update.path().to_path_buf(),
                source,
            })?;
            if current != update.original_bytes() {
                return Err(TasksAuthoringError::StalePlan {
                    path: update.path().to_path_buf(),
                    detail: "work-package prompt changed after the plan was built".to_owned(),
                });
            }
        }

        let mut written: Vec<PathBuf> = Vec::new();
        for update in &self.updates {
            if let Err(source) = observer.before_write(update.path()) {
                return Err(rollback_or_error(
                    rollback_updates(&written, &self.updates),
                    TasksAuthoringError::Write {
                        path: update.path().to_path_buf(),
                        source,
                    },
                ));
            }
            // Include the current file in rollback even if atomic_write fails
            // after rename (AtomicCommitIndeterminate).
            written.push(update.path().to_path_buf());
            if let Err(source) = atomic_write(update.path(), update.updated_bytes()) {
                return Err(rollback_or_error(
                    rollback_updates(&written, &self.updates),
                    TasksAuthoringError::Kernel {
                        path: update.path().to_path_buf(),
                        source,
                    },
                ));
            }
        }

        Ok(DependencyApplyReceipt {
            updated: written,
            preserved: self.preserved_wps.clone(),
            unchanged: self.unchanged_wps.clone(),
        })
    }
}

fn rollback_updates(written: &[PathBuf], updates: &[DependencyUpdate]) -> Result<(), String> {
    let mut failure = None;
    for path in written.iter().rev() {
        if let Some(update) = updates.iter().find(|update| update.path() == path)
            && let Err(error) = atomic_write(path, update.original_bytes())
        {
            failure = Some(error.to_string());
        }
    }
    match failure {
        Some(detail) => Err(detail),
        None => Ok(()),
    }
}

fn rollback_or_error(
    rollback: Result<(), String>,
    original: TasksAuthoringError,
) -> TasksAuthoringError {
    match rollback {
        Ok(()) => original,
        Err(detail) => TasksAuthoringError::RollbackFailed {
            detail: format!("{original}; rollback also failed: {detail}"),
        },
    }
}

/// Result of applying a finalization plan.
#[derive(Clone, Debug)]
pub struct DependencyApplyReceipt {
    updated: Vec<PathBuf>,
    preserved: Vec<String>,
    unchanged: Vec<String>,
}

impl DependencyApplyReceipt {
    /// Prompt files rewritten.
    pub fn updated(&self) -> &[PathBuf] {
        &self.updated
    }

    /// WPs whose frontmatter dependency value was preserved.
    pub fn preserved(&self) -> &[String] {
        &self.preserved
    }

    /// WPs that needed no change.
    pub fn unchanged(&self) -> &[String] {
        &self.unchanged
    }
}

struct WorkPackageFrontmatter {
    path: PathBuf,
    source: String,
    dependencies: Vec<WpId>,
}

fn discover_work_packages(
    tasks_dir: &Path,
) -> Result<BTreeMap<WpId, PathBuf>, TasksAuthoringError> {
    let mut files = BTreeMap::new();
    let entries = fs::read_dir(tasks_dir).map_err(|source| TasksAuthoringError::Io {
        action: "list",
        path: tasks_dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| TasksAuthoringError::Io {
            action: "list",
            path: tasks_dir.to_path_buf(),
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| TasksAuthoringError::Io {
                action: "inspect",
                path: entry.path(),
                source,
            })?;
        if file_type.is_symlink() {
            return Err(TasksAuthoringError::SymlinkedEntry { path: entry.path() });
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(TasksAuthoringError::NonUtf8FileName { path: entry.path() });
        };
        if !name.ends_with(".md") {
            continue;
        }
        let Some(prefix) = name.strip_prefix("WP") else {
            continue;
        };
        let digits: String = prefix.chars().take_while(char::is_ascii_digit).collect();
        if digits.len() < 2 {
            continue;
        }
        let wp_id = WpId::from_str(&format!("WP{digits}")).map_err(|error| {
            TasksAuthoringError::InvalidFrontmatter {
                path: entry.path(),
                detail: error.to_string(),
            }
        })?;
        if let Some(previous) = files.insert(wp_id.clone(), entry.path()) {
            return Err(TasksAuthoringError::DuplicateWorkPackage {
                wp: wp_id,
                first: previous,
                second: entry.path(),
            });
        }
    }
    Ok(files)
}

fn read_frontmatter(
    path: &Path,
    expected: &WpId,
) -> Result<WorkPackageFrontmatter, TasksAuthoringError> {
    let bytes = read_bounded(path, DEFAULT_WORK_PACKAGE_BYTE_LIMIT)
        .map_err(|source| map_read_error(path, source))?;
    let source =
        std::str::from_utf8(&bytes).map_err(|error| TasksAuthoringError::InvalidFrontmatter {
            path: path.to_path_buf(),
            detail: format!("not valid UTF-8: {error}"),
        })?;
    let frontmatter =
        split_frontmatter(source).map_err(|error| TasksAuthoringError::InvalidFrontmatter {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    for field in ["work_package_id", "dependencies"] {
        if count_top_level_field(frontmatter.yaml, field) > 1 {
            return Err(TasksAuthoringError::InvalidFrontmatter {
                path: path.to_path_buf(),
                detail: format!("frontmatter declares {field} more than once"),
            });
        }
    }
    let mapping: Mapping =
        decode_mapping(frontmatter.yaml).map_err(|source| map_patch_error(path, source))?;
    let id = mapping
        .get(Value::String("work_package_id".to_owned()))
        .and_then(Value::as_str)
        .ok_or_else(|| TasksAuthoringError::InvalidFrontmatter {
            path: path.to_path_buf(),
            detail: "missing work_package_id".to_owned(),
        })?;
    let parsed = WpId::from_str(id).map_err(|error| TasksAuthoringError::InvalidFrontmatter {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    if &parsed != expected {
        return Err(TasksAuthoringError::WorkPackageIdMismatch {
            path: path.to_path_buf(),
            filename: expected.to_string(),
            frontmatter: parsed.to_string(),
        });
    }
    let dependencies = sequence_of(&mapping, "dependencies")
        .map_err(|source| map_patch_error(path, source))?
        .unwrap_or_default()
        .into_iter()
        .map(|value| {
            WpId::from_str(&value).map_err(|error| TasksAuthoringError::InvalidFrontmatter {
                path: path.to_path_buf(),
                detail: format!("invalid dependency {value:?}: {error}"),
            })
        })
        .collect::<Result<Vec<WpId>, _>>()?;
    let mut seen = BTreeSet::new();
    for dependency in &dependencies {
        if !seen.insert(dependency.clone()) {
            return Err(TasksAuthoringError::InvalidFrontmatter {
                path: path.to_path_buf(),
                detail: format!("dependency {dependency} is declared more than once"),
            });
        }
    }

    Ok(WorkPackageFrontmatter {
        path: path.to_path_buf(),
        source: source.to_owned(),
        dependencies,
    })
}

fn validate_dependency_graph(graph: &BTreeMap<WpId, Vec<WpId>>) -> Result<(), TasksAuthoringError> {
    for (wp, dependencies) in graph {
        for dependency in dependencies {
            if dependency == wp {
                return Err(TasksAuthoringError::SelfDependency { wp: wp.clone() });
            }
            if !graph.contains_key(dependency) {
                return Err(TasksAuthoringError::UnknownDependency {
                    wp: wp.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    fn visit(
        id: &WpId,
        graph: &BTreeMap<WpId, Vec<WpId>>,
        colors: &mut BTreeMap<WpId, Color>,
        stack: &mut Vec<WpId>,
    ) -> Result<(), TasksAuthoringError> {
        colors.insert(id.clone(), Color::Gray);
        stack.push(id.clone());
        let mut dependencies = graph.get(id).cloned().unwrap_or_default();
        dependencies.sort();
        for dependency in dependencies {
            match colors.get(&dependency).copied().unwrap_or(Color::White) {
                Color::Gray => {
                    let start = stack
                        .iter()
                        .position(|entry| entry == &dependency)
                        .unwrap_or(0);
                    let mut path: Vec<String> =
                        stack[start..].iter().map(ToString::to_string).collect();
                    path.push(dependency.to_string());
                    return Err(TasksAuthoringError::DependencyCycle {
                        cycle: path.join(" -> "),
                    });
                }
                Color::White => visit(&dependency, graph, colors, stack)?,
                Color::Black => {}
            }
        }
        stack.pop();
        colors.insert(id.clone(), Color::Black);
        Ok(())
    }

    let mut colors: BTreeMap<WpId, Color> =
        graph.keys().cloned().map(|id| (id, Color::White)).collect();
    for id in graph.keys() {
        if colors.get(id).copied() == Some(Color::White) {
            let mut stack = Vec::new();
            visit(id, graph, &mut colors, &mut stack)?;
        }
    }
    Ok(())
}

fn set_of(ids: &[WpId]) -> BTreeSet<WpId> {
    ids.iter().cloned().collect()
}

fn project_root_for(tasks_dir: &Path) -> Result<PathBuf, TasksAuthoringError> {
    let mission = tasks_dir
        .parent()
        .ok_or_else(|| TasksAuthoringError::StalePlan {
            path: tasks_dir.to_path_buf(),
            detail: "tasks directory has no Mission parent".to_owned(),
        })?;
    let specs = mission
        .parent()
        .ok_or_else(|| TasksAuthoringError::StalePlan {
            path: tasks_dir.to_path_buf(),
            detail: "Mission directory has no kitty-specs parent".to_owned(),
        })?;
    let root = specs
        .parent()
        .ok_or_else(|| TasksAuthoringError::StalePlan {
            path: tasks_dir.to_path_buf(),
            detail: "kitty-specs has no project-root parent".to_owned(),
        })?;
    Ok(root.to_path_buf())
}

fn map_read_error(path: &Path, source: sk_kernel::KernelError) -> TasksAuthoringError {
    match source {
        sk_kernel::KernelError::TooLarge { limit, .. } => TasksAuthoringError::TooLarge {
            path: path.to_path_buf(),
            limit,
        },
        other => TasksAuthoringError::Kernel {
            path: path.to_path_buf(),
            source: other,
        },
    }
}

fn map_patch_error(path: &Path, source: PatchError) -> TasksAuthoringError {
    match source {
        PatchError::MissingFrontmatter => TasksAuthoringError::InvalidFrontmatter {
            path: path.to_path_buf(),
            detail: "missing YAML frontmatter".to_owned(),
        },
        PatchError::Unterminated => TasksAuthoringError::InvalidFrontmatter {
            path: path.to_path_buf(),
            detail: "unterminated YAML frontmatter".to_owned(),
        },
        PatchError::InvalidYaml(detail) | PatchError::UnsafeRewrite(detail) => {
            TasksAuthoringError::Yaml {
                path: path.to_path_buf(),
                detail,
            }
        }
        PatchError::DuplicateField(field) => TasksAuthoringError::InvalidFrontmatter {
            path: path.to_path_buf(),
            detail: format!("duplicate {field} field"),
        },
    }
}

/// Typed task-outline and finalization failures.
#[derive(Debug, Error)]
pub enum TasksAuthoringError {
    /// `tasks.md` does not exist.
    #[error("tasks.md not found: {path}")]
    MissingTasksFile { path: PathBuf },

    /// The tasks directory does not exist.
    #[error("tasks directory not found: {path}")]
    MissingTasksDirectory { path: PathBuf },

    /// A bounded read exceeded its limit.
    #[error("file {path} is larger than the {limit}-byte limit")]
    TooLarge { path: PathBuf, limit: u64 },

    /// A file is not valid UTF-8.
    #[error("file {path} is not valid UTF-8: {detail}")]
    InvalidUtf8 { path: PathBuf, detail: String },

    /// `tasks.md` declares the same work-package section twice.
    #[error("tasks.md declares work package {wp} more than once")]
    DuplicateSection { wp: WpId },

    /// `tasks.md` sections and `tasks/` prompt files do not match.
    #[error(
        "tasks.md work-package coverage is incomplete: missing sections {missing_sections:?}, extra sections {extra_sections:?}"
    )]
    Coverage {
        missing_sections: Vec<String>,
        extra_sections: Vec<String>,
    },

    /// A work-package prompt has malformed or missing frontmatter.
    #[error("work package file {path} has invalid frontmatter: {detail}")]
    InvalidFrontmatter { path: PathBuf, detail: String },

    /// Frontmatter YAML could not be decoded.
    #[error("work package file {path} has invalid YAML: {detail}")]
    Yaml { path: PathBuf, detail: String },

    /// The filename id and frontmatter id disagree.
    #[error("work package file {path} has filename id {filename} but frontmatter id {frontmatter}")]
    WorkPackageIdMismatch {
        path: PathBuf,
        filename: String,
        frontmatter: String,
    },

    /// Two prompt files claim the same work-package id.
    #[error("work package {wp} is declared by both {first} and {second}")]
    DuplicateWorkPackage {
        wp: WpId,
        first: PathBuf,
        second: PathBuf,
    },

    /// A prompt file is a symlink.
    #[error("refusing to follow symlink {path}")]
    SymlinkedEntry { path: PathBuf },

    /// A tasks-directory entry has a non-UTF-8 name.
    #[error("tasks entry {path} has a non-UTF-8 filename")]
    NonUtf8FileName { path: PathBuf },

    /// tasks.md and frontmatter disagree about a work package's dependencies.
    #[error(
        "dependency disagreement for {wp}: frontmatter has {existing:?}, tasks.md parsed {parsed:?}; resolve the disagreement before finalizing"
    )]
    DependencyConflict {
        wp: String,
        existing: Vec<String>,
        parsed: Vec<String>,
    },

    /// A work package depends on itself.
    #[error("work package {wp} depends on itself")]
    SelfDependency { wp: WpId },

    /// A work package depends on an id with no prompt file.
    #[error("work package {wp} depends on unknown work package {dependency}")]
    UnknownDependency { wp: WpId, dependency: WpId },

    /// The effective dependency graph contains a cycle.
    #[error("dependency cycle detected: {cycle}")]
    DependencyCycle { cycle: String },

    /// The plan no longer matches the filesystem.
    #[error("stale finalization plan for {path}: {detail}")]
    StalePlan { path: PathBuf, detail: String },

    /// The write lock could not be acquired.
    #[error("cannot acquire Mission write lock {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: sk_kernel::KernelError,
    },

    /// A filesystem operation failed.
    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A kernel primitive refused a read or write.
    #[error("kernel refused {path}: {source}")]
    Kernel {
        path: PathBuf,
        #[source]
        source: sk_kernel::KernelError,
    },

    /// A planned prompt write failed.
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A failed finalization could not be fully rolled back.
    #[error("finalization rollback failed: {detail}")]
    RollbackFailed { detail: String },
}

const _: (&str, &str) = (TASKS_FILE_NAME, TASKS_DIR_NAME);
